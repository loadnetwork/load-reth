# Contributing to load-reth

Thank you for your interest in contributing to load-reth!

## Development Setup

### Prerequisites

- Rust 1.82+ (check `rust-toolchain.toml` or run `rustup show`)
- Docker with buildx (for container builds)
- `cross` tool for cross-compilation: `cargo install cross`

### Quick Start

```bash
# Clone the repository
git clone https://github.com/loadnetwork/load-reth
cd load-reth

# Build
make build

# Run tests
make test

# Run all linters
make lint

# Run full PR checks locally
make pr
```

## Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/). PR titles and commits should follow this format:

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Formatting, missing semicolons, etc.
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `perf`: Performance improvement
- `test`: Adding or updating tests
- `build`: Build system or external dependencies
- `ci`: CI configuration
- `chore`: Other changes that don't modify src or test files

**Examples:**
```
feat(rpc): add blob submission endpoint
fix(consensus): correct blob commitment validation
docs: update README with docker instructions
```

## Pull Request Process

1. Fork the repository and create your branch from `main`
2. Run `make pr` locally to ensure all checks pass
3. Update documentation if needed
4. Submit a PR with a clear description

### PR Checklist

- [ ] Code compiles without warnings (`make clippy`)
- [ ] Code is formatted (`make fmt`)
- [ ] Dependencies are sorted (`make sort`)
- [ ] Tests pass (`make test`)
- [ ] Documentation builds (`make docs`)
- [ ] Security checks pass (`make deny` and `make audit`)

## Code Style

- Follow Rust idioms and best practices
- Use `rustfmt` (nightly) for formatting
- Address all `clippy` warnings
- Keep dependencies sorted with `cargo-sort`

## Docker Builds

```bash
# Local build
make docker-build-local

# Debug build (fast, in-container compilation)
make docker-build-debug

# Multi-arch build (requires cross)
make docker-build-push-latest
```

## Questions?

Open an issue or reach out to the maintainers.
