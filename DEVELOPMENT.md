# Development Guide

This guide will help you set up your development environment for working with the CDK repository.

## Prerequisites

Before you begin, ensure you have:
- Git installed on your system
- GitHub account
- Basic familiarity with command line operations

## Initial Setup

### 1. Fork and Clone the Repository

1. Navigate to the CDK repository on GitHub
2. Click the "Fork" button in the top-right corner
3. Clone your forked repository:
```bash
git clone https://github.com/YOUR-USERNAME/cdk.git
cd cdk
```

### 2. Install Nix

<!-- 
MIT License

Copyright (c) 2021 elsirion
https://github.com/fedimint/fedimint/blob/master/docs/dev-env.md
-->

CDK uses [Nix](https://nixos.org/explore.html) for building, CI, and managing dev environment.
Note: only `Nix` (the language & package manager) and not the NixOS (the Linux distribution) is needed.
Nix can be installed on any Linux distribution and macOS.

While it is technically possible to not use Nix, it is highly recommended as
it ensures consistent and reproducible environment for all developers.

### Install Nix

You have 2 options to install nix:

* **RECOMMENDED:** The [Determinate Nix Installer](https://github.com/DeterminateSystems/nix-installer)
* [The official installer](https://nixos.org/download.html)

Example:

```
> nix --version
nix (Nix) 2.9.1
```

The exact version might be different.

### Enable nix flakes

If you installed Nix using the "determinate installer" you can skip this step. If you used the "official installer", edit either `~/.config/nix/nix.conf` or `/etc/nix/nix.conf` and add:

```
experimental-features = nix-command flakes
```

If the Nix installation is in multi-user mode, don’t forget to restart the nix-daemon.

## Use Nix Shell

```sh
  nix develop -c $SHELL  
```

## Common Development Tasks

### Building the Project
```sh
just build
```

### Running Unit Tests
```bash
just test
```

### Running Integration Tests
```bash
just itest REDB/SQLITE/MEMEORY
```

### Running Format
```bash
just format
```


### Running Clippy
```bash
just clippy
```

### Running final check before commit
```sh
just final-check
```


## Best Practices

1. **Branch Management**
   - Create feature branches from `main`
   - Use descriptive branch names: `feature/new-feature` or `fix/bug-description`

2. **Commit Messages**
   - Follow conventional commits format
   - Begin with type: `feat:`, `fix:`, `docs:`, `chore:`, etc.
   - Provide clear, concise descriptions

3. **Testing**
   - Write tests for new features
   - Ensure all tests pass before submitting PR
   - Include integration tests where applicable

## Troubleshooting

### Common Issues

1. **Development Shell Issues**
   - Clean Nix store: `nix-collect-garbage -d`
   - Remove and recreate development shell

### Getting Help

- Open an issue on GitHub
- Check existing issues for similar problems
- Include relevant error messages and system information
- Reach out in Matrix [Invite link](https://matrix.to/#/#dev:matrix.cashu.space)

## Contributing

1. Create a feature branch
2. Make your changes
3. Run tests and formatting
4. Submit a pull request
5. Wait for review and address feedback

## Additional Resources

- [Nix Documentation](https://nixos.org/manual/nix/stable/)
- [Contributing Guidelines](CONTRIBUTING.md)

## License

Refer to the LICENSE file in the repository for terms of use and distribution.
