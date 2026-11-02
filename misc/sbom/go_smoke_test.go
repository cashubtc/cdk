package cdk_ffi

import "testing"

// This smoke test is copied into private staging, never the user's package.
func TestSbomNativeLoad(t *testing.T) {
	mnemonic, err := GenerateMnemonic()
	if err != nil {
		t.Fatalf("native mnemonic generation failed: %v", err)
	}
	if mnemonic == "" {
		t.Fatal("native mnemonic generation returned an empty result")
	}
}
