# Quality checks

`make quality` runs formatting, strict Clippy and C smoke-source analysis,
Doxygen, and warning-free Rustdoc. `make test` runs Rust and C ABI smoke
tests. `make coverage` verifies 100% line and branch coverage for production
Rust sources using the pinned coverage toolchain. `make ci` adds staged
installation, Debian package, source archive, and unpacked-archive checks.

The optional `make container-coverage` target rebuilds a labeled, disposable
Debian 13 quality container and removes project-owned stale test containers on
exit. Its source-owned image installs the released squelch, CTCSS and DCS
development/runtime packages from the signed USBRadioPlus APT repository:
squelch and CTCSS at `0.1.0~alpha2-1`, DCS at `0.1.0~alpha1-1`. No provider
source is built into the image. It does not alter source files.
