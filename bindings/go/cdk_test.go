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
