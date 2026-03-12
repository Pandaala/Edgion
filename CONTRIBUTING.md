# Contributing to Edgion

First off, thank you for considering contributing to Edgion! 🎉

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [How to Contribute](#how-to-contribute)
- [Pull Request Process](#pull-request-process)
- [Coding Guidelines](#coding-guidelines)
- [Commit Message Guidelines](#commit-message-guidelines)

## Code of Conduct

This project adheres to a Code of Conduct. By participating, you are expected to uphold this code. Please report unacceptable behavior to the project maintainers.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/Edgion.git`
3. Add upstream remote: `git remote add upstream https://github.com/Pandaala/Edgion.git`

## Development Setup

### Prerequisites

- Rust 1.75 or higher
- CMake
- libclang-dev
- protobuf-compiler

### Build

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt-get install -y cmake libclang-dev protobuf-compiler

# Build all binaries
cargo build

# Run tests
cargo test --all

# Run clippy (linter)
cargo clippy --all-targets --all-features -- -D warnings

# Format code
cargo fmt --all
```

### Running Integration Tests

```bash
cd examples/testing
./run_integration_test.sh
```

## How to Contribute

### Reporting Bugs

- Use the [Bug Report template](.github/ISSUE_TEMPLATE/bug_report.yml)
- Include detailed steps to reproduce
- Include environment details (OS, Rust version, etc.)

### Suggesting Features

- Use the [Feature Request template](.github/ISSUE_TEMPLATE/feature_request.yml)
- Describe the use case and expected behavior
- Check existing issues first to avoid duplicates

### Code Contributions

1. Check [open issues](https://github.com/Pandaala/Edgion/issues) for tasks
2. Comment on an issue to claim it
3. Create a branch from `main`
4. Make your changes
5. Submit a Pull Request

## Pull Request Process

1. **Create a feature branch**
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes**
   - Follow the coding guidelines
   - Add tests for new functionality
   - Update documentation if needed

3. **Test your changes**
   ```bash
   cargo test --all
   cargo clippy --all-targets --all-features -- -D warnings
   cargo fmt --all --check
   ```

4. **Commit with clear messages**
   ```bash
   git commit -m "feat: add new feature description"
   ```

5. **Push and create PR**
   ```bash
   git push origin feature/your-feature-name
   ```

6. **PR Review**
   - Ensure CI passes
   - Address review comments
   - Squash commits if requested

## Coding Guidelines

### Rust Style

- Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Write documentation for public APIs

### Code Organization

```
src/
├── bin/           # Binary entry points
├── core/          # Core functionality
│   ├── api/       # Admin API
│   ├── backends/  # Backend management
│   ├── gateway/   # Gateway logic
│   ├── plugins/   # Plugin system
│   └── routes/    # Route handlers
├── types/         # Type definitions
└── lib.rs         # Library entry
```

### Testing

- Write unit tests in the same file using `#[cfg(test)]`
- Integration tests go in `examples/testing/`
- Aim for meaningful test coverage

## Commit Message Guidelines

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `style` | Code style (formatting) |
| `refactor` | Code refactoring |
| `perf` | Performance improvement |
| `test` | Adding tests |
| `chore` | Maintenance tasks |
| `ci` | CI/CD changes |

### Examples

```
feat(gateway): add WebSocket protocol support
fix(tls): resolve certificate reload issue
docs: update installation guide
refactor(plugins): simplify filter chain logic
```

## Questions?

- Open a [Discussion](https://github.com/Pandaala/Edgion/discussions)
- Check existing [Documentation](docs/)

Thank you for contributing! 🚀

