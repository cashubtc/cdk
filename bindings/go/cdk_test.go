package cdk_ffi

import (
	"fmt"
	"os"
	"sync"
	"testing"
	"time"
)

const testMintUrl = "https://testnut.cashudevkit.org"

func newTestWallet(t *testing.T, path string) *Wallet {
	t.Helper()
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		t.Fatalf("GenerateMnemonic: %v", err)
	}
	w, err := NewWallet(
		testMintUrl,
		CurrencyUnitSat{},
		mnemonic,
		WalletStoreSqlite{Path: path},
		WalletConfig{TargetProofCount: nil},
	)
	if err != nil {
		t.Fatalf("NewWallet: %v", err)
	}
	return w
}

func tempDBPath(t *testing.T) string {
	t.Helper()
	f, err := os.CreateTemp("", "cdk_test_*.sqlite")
	if err != nil {
		t.Fatalf("CreateTemp: %v", err)
	}
	path := f.Name()
	f.Close()
	t.Cleanup(func() { os.Remove(path) })
	return path
}

func TestInitialBalanceIsZero(t *testing.T) {
	w := newTestWallet(t, tempDBPath(t))
	defer w.Destroy()

	balance, err := w.TotalBalance()
	if err != nil {
		t.Fatalf("TotalBalance: %v", err)
	}
	if balance.Value != 0 {
		t.Errorf("expected zero balance, got %d", balance.Value)
	}
}

func TestInMemorySqliteConcurrentAccess(t *testing.T) {
	w := newTestWallet(t, ":memory:")
	defer w.Destroy()

	var wg sync.WaitGroup
	errs := make(chan error, 64)

	for i := 0; i < 64; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			balance, err := w.TotalBalance()
			if err != nil {
				errs <- err
				return
			}
			if balance.Value != 0 {
				errs <- fmt.Errorf("expected zero balance, got %d", balance.Value)
			}
		}()
	}

	wg.Wait()
	close(errs)

	for err := range errs {
		t.Error(err)
	}
}

func newRateLimitedWallet(rateLimit *RateLimit) (*Wallet, error) {
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		return nil, err
	}
	return NewWallet(
		"https://mint.example.com",
		CurrencyUnitSat{},
		mnemonic,
		WalletStoreSqlite{Path: ":memory:"},
		WalletConfig{TargetProofCount: nil, RateLimit: rateLimit},
	)
}

func TestRateLimitDefaultsToPacing(t *testing.T) {
	// A nil rate limit is the backwards-compatible spelling and must keep
	// selecting the built-in default.
	omitted, err := newRateLimitedWallet(nil)
	if err != nil {
		t.Fatalf("NewWallet: %v", err)
	}
	defer omitted.Destroy()
	if !omitted.IsRateLimited() {
		t.Error("an omitted rate limit should pace by default")
	}

	var def RateLimit = RateLimitDefault{}
	explicit, err := newRateLimitedWallet(&def)
	if err != nil {
		t.Fatalf("NewWallet: %v", err)
	}
	defer explicit.Destroy()
	if !explicit.IsRateLimited() {
		t.Error("Default should pace the wallet")
	}
}

func TestRateLimitCanBeDisabledAndReEnabled(t *testing.T) {
	var disabled RateLimit = RateLimitDisabled{}
	w, err := newRateLimitedWallet(&disabled)
	if err != nil {
		t.Fatalf("NewWallet: %v", err)
	}
	defer w.Destroy()

	if w.IsRateLimited() {
		t.Error("Disabled should not pace")
	}

	if err := w.SetRateLimit(RateLimitDefault{}); err != nil {
		t.Fatalf("SetRateLimit: %v", err)
	}
	if !w.IsRateLimited() {
		t.Error("a wallet built disabled should be re-enablable")
	}
}

func TestCustomRateLimitPaces(t *testing.T) {
	var custom RateLimit = RateLimitCustom{Capacity: 5, RefillPerMinute: 30}
	w, err := newRateLimitedWallet(&custom)
	if err != nil {
		t.Fatalf("NewWallet: %v", err)
	}
	defer w.Destroy()

	if !w.IsRateLimited() {
		t.Error("Custom should pace the wallet")
	}
}

func TestZeroRateLimitValuesAreRejected(t *testing.T) {
	for _, rl := range []RateLimit{
		RateLimitCustom{Capacity: 0, RefillPerMinute: 30},
		RateLimitCustom{Capacity: 5, RefillPerMinute: 0},
	} {
		rl := rl
		if w, err := newRateLimitedWallet(&rl); err == nil {
			w.Destroy()
			t.Errorf("%v should be rejected", rl)
		}
	}
}

func TestMintFlow(t *testing.T) {
	w := newTestWallet(t, tempDBPath(t))
	defer w.Destroy()

	amount := Amount{Value: 100}
	quote, err := w.MintQuote(PaymentMethodBolt11{}, &amount, nil, nil)
	if err != nil {
		t.Fatalf("MintQuote: %v", err)
	}
	if quote.Id == "" {
		t.Fatal("expected non-empty quote ID")
	}
	if quote.Request == "" {
		t.Fatal("expected non-empty payment request")
	}

	// testnut pays quotes automatically
	time.Sleep(3 * time.Second)

	proofs, err := w.Mint(quote.Id, SplitTargetNone{}, nil)
	if err != nil {
		t.Fatalf("Mint: %v", err)
	}
	if len(proofs) == 0 {
		t.Fatal("expected proofs")
	}

	balance, err := w.TotalBalance()
	if err != nil {
		t.Fatalf("TotalBalance: %v", err)
	}
	if balance.Value != 100 {
		t.Errorf("expected balance 100, got %d", balance.Value)
	}
}
