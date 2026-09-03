package cdk_ffi

import (
	"errors"
	"fmt"
	"os"
	"sync"
	"testing"
)

const testMintURL = "https://mint.example.com"

func newTestWallet(t *testing.T, path string) *Wallet {
	t.Helper()
	return newTestWalletWithConfig(t, path, nil)
}

func newTestWalletWithConfig(t *testing.T, path string, config *WalletConfig) *Wallet {
	t.Helper()
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		t.Fatalf("GenerateMnemonic: %v", err)
	}
	wallet, err := WalletOpen(WalletOpenRequest{
		MintUrl:  testMintURL,
		Unit:     CurrencyUnitSat{},
		Mnemonic: mnemonic,
		Store:    WalletStoreSqlite{Path: path},
		Config:   config,
	})
	if err != nil {
		t.Fatalf("WalletOpen: %v", err)
	}
	return wallet
}

func tempDBPath(t *testing.T) string {
	t.Helper()
	file, err := os.CreateTemp("", "cdk_test_*.sqlite")
	if err != nil {
		t.Fatalf("CreateTemp: %v", err)
	}
	path := file.Name()
	if err := file.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	t.Cleanup(func() { _ = os.Remove(path) })
	return path
}

func TestWalletOpenProvidesLocalState(t *testing.T) {
	wallet := newTestWallet(t, tempDBPath(t))
	defer wallet.Destroy()

	identity := wallet.Identity()
	if identity.MintUrl.Url != testMintURL {
		t.Errorf("expected mint URL %q, got %q", testMintURL, identity.MintUrl.Url)
	}
	if _, ok := identity.Unit.(CurrencyUnitSat); !ok {
		t.Errorf("expected sat unit, got %T", identity.Unit)
	}

	balance, err := wallet.Balance()
	if err != nil {
		t.Fatalf("Balance: %v", err)
	}
	if balance.Available.Value != 0 || balance.Pending.Value != 0 || balance.Reserved.Value != 0 {
		t.Errorf("expected zero balance, got %+v", balance)
	}

	report, err := wallet.Synchronize(SyncPolicyLocalOnly)
	if err != nil {
		t.Fatalf("Synchronize(LocalOnly): %v", err)
	}
	if report.Wallet.MintUrl.Url != testMintURL || report.FailedOperations != 0 {
		t.Errorf("unexpected local sync report: %+v", report)
	}
}

func TestInMemorySqliteConcurrentBalanceReads(t *testing.T) {
	wallet := newTestWallet(t, ":memory:")
	defer wallet.Destroy()

	var waitGroup sync.WaitGroup
	errs := make(chan error, 64)
	for range 64 {
		waitGroup.Add(1)
		go func() {
			defer waitGroup.Done()
			balance, err := wallet.Balance()
			if err != nil {
				errs <- err
				return
			}
			if balance.Available.Value != 0 {
				errs <- fmt.Errorf("expected zero available balance, got %d", balance.Available.Value)
			}
		}()
	}

	waitGroup.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}

func TestCashuWalletCoordinatesMultipleMints(t *testing.T) {
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		t.Fatalf("GenerateMnemonic: %v", err)
	}
	root, err := CashuWalletOpen(CashuWalletOpenRequest{
		Mnemonic: mnemonic,
		Store:    WalletStoreSqlite{Path: tempDBPath(t)},
	})
	if err != nil {
		t.Fatalf("CashuWalletOpen: %v", err)
	}
	defer root.Destroy()

	first, err := root.Wallet(MintUrl{Url: "https://one.example.com"}, CurrencyUnitSat{})
	if err != nil {
		t.Fatalf("first wallet: %v", err)
	}
	defer first.Destroy()
	second, err := root.Wallet(MintUrl{Url: "https://two.example.com"}, CurrencyUnitSat{})
	if err != nil {
		t.Fatalf("second wallet: %v", err)
	}
	defer second.Destroy()

	balances, err := root.Balances()
	if err != nil {
		t.Fatalf("Balances: %v", err)
	}
	if len(balances) != 2 {
		t.Fatalf("expected two wallet balances, got %d", len(balances))
	}
}

func TestRateLimitConfigurationIsValidatedAtOpen(t *testing.T) {
	for name, rateLimit := range map[string]RateLimit{
		"default":  RateLimitDefault{},
		"disabled": RateLimitDisabled{},
		"custom":   RateLimitCustom{Capacity: 5, RefillPerMinute: 30},
	} {
		t.Run(name, func(t *testing.T) {
			wallet := newTestWalletWithConfig(t, tempDBPath(t), &WalletConfig{RateLimit: &rateLimit})
			wallet.Destroy()
		})
	}

	var invalid RateLimit = RateLimitCustom{Capacity: 0, RefillPerMinute: 30}
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		t.Fatalf("GenerateMnemonic: %v", err)
	}
	_, err = WalletOpen(WalletOpenRequest{
		MintUrl:  testMintURL,
		Unit:     CurrencyUnitSat{},
		Mnemonic: mnemonic,
		Store:    WalletStoreSqlite{Path: tempDBPath(t)},
		Config:   &WalletConfig{RateLimit: &invalid},
	})
	assertInvalidInput(t, err)
}

func TestInvalidOperationIDReturnsStructuredError(t *testing.T) {
	wallet := newTestWallet(t, tempDBPath(t))
	defer wallet.Destroy()

	_, err := wallet.SendPlan("not-an-operation-id")
	assertInvalidInput(t, err)
}

func assertInvalidInput(t *testing.T, err error) {
	t.Helper()
	if err == nil {
		t.Fatal("expected an invalid-input error")
	}
	var cdkErr *FfiErrorCdk
	if !errors.As(err, &cdkErr) {
		t.Fatalf("expected FfiErrorCdk, got %T: %v", err, err)
	}
	if cdkErr.Kind != WalletErrorKindInvalidInput || cdkErr.Code != 40000 || cdkErr.Retryable {
		t.Errorf("unexpected structured error: %+v", cdkErr)
	}
}
