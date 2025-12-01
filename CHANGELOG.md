# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial load-reth implementation based on reth SDK v1.9.3
- Custom blob validation for Load Network
- Docker multi-arch support (amd64, arm64)
- CI/CD workflows with conventional commits enforcement
- Security scanning with cargo-deny, cargo-audit, and Trivy

### Changed
- Upgraded to reth SDK v1.9.3 (pulls in patched tracing-subscriber and aligns with upstream SDK changes).

### Deprecated
- N/A

### Removed
- N/A

### Fixed
- N/A

### Security
- N/A
