# cdk-go

Go FFI bindings for [cashubtc/cdk](https://github.com/cashubtc/cdk) (Cashu Development Kit), generated via [uniffi-bindgen-go](https://github.com/NordSecurity/uniffi-bindgen-go).

Prebuilt native libraries are included — downstream consumers only need Go.

## Install

```bash
go get github.com/cashubtc/cdk-go
```

## Quick Start

```go
package main

import (
	"fmt"

	cdk "github.com/cashubtc/cdk-go/bindings/cdkffi"
)

func main() {
	// Create a new wallet
	seed := cdk.GenerateMnemonic()
	fmt.Println("Mnemonic:", seed)
}
```

> **Note:** Requires `CGO_ENABLED=1` (the default on most systems).

## Supported platforms

| OS      | Arch  | Library            |
|---------|-------|--------------------|
| Linux   | amd64 | `libcdk_ffi.so`    |
| Linux   | arm64 | `libcdk_ffi.so`    |
| macOS   | arm64 | `libcdk_ffi.dylib` |
| macOS   | amd64 | `libcdk_ffi.dylib` |
| Windows | amd64 | `cdk_ffi.dll`      |

CGO link flags are automatically selected per platform via build tags. No manual setup required.

## Prerequisites

- Go 1.22+
- `CGO_ENABLED=1`

## Building from Source

Requires Rust and the [just](https://github.com/casey/just) command runner.

```bash
# Generate Go bindings and build native library
just binding-go

# Run tests
just test-go
```

## CI/CD — Publishing Workflow

The `go-publish.yml` workflow (in the CDK monorepo) builds native binaries,
syncs sources to `cdk-go`, and creates a tagged release. The following secrets
and variables must be configured in the **CDK monorepo** repository settings
(Settings → Secrets and variables → Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos. Used to clone, push, and create releases. Shared across all FFI publish workflows. |

#### How to create the PAT

1. Go to **GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens**.
2. Create a token scoped to the FFI target repositories with **Contents** (read/write) and **Metadata** (read) permissions.
3. Add it as a repository secret named `FFI_DEPLOY_KEY` in the monorepo.

### Variables

| Name | Purpose | Example |
|---|---|---|
| `CDK_GO_REPO` | Owner/repo of the target Go package repository. | `cashubtc/cdk-go` |

Set this under **Settings → Secrets and variables → Actions → Variables**.

## License

[MIT](https://github.com/cashubtc/cdk/blob/main/LICENSE)
