.DEFAULT_GOAL := all

CARGO ?= cargo
CARGO_FMT ?= cargo fmt
CARGO_CLIPPY ?= cargo clippy
CARGO_LLVM_COV ?= cargo llvm-cov
DOXYGEN ?= doxygen
SHELLCHECK ?= shellcheck
CPPCHECK ?= cppcheck
READELF ?= readelf
CC ?= cc
PYTHON ?= python3
PKG_CONFIG ?= pkg-config

PACKAGE := librptadvradio
CRATE := rptadvradio
PACKAGE_VERSION ?= 0.1.0-alpha.3
SOVERSION := 3
PREFIX ?= /usr/local
DESTDIR ?=
LIBDIR ?= $(PREFIX)/lib
CARGO_TARGET_DIR ?= target
TARGET_RELEASE := $(CARGO_TARGET_DIR)/release
LIBRARY_BASENAME := lib$(CRATE)
TARGET_LIBRARY := $(TARGET_RELEASE)/$(LIBRARY_BASENAME).so
LIBRARY_VERSIONED := build/$(LIBRARY_BASENAME).so.$(SOVERSION).$(PACKAGE_VERSION)
LIBRARY_SONAME := build/$(LIBRARY_BASENAME).so.$(SOVERSION)
LIBRARY_LINK := build/$(LIBRARY_BASENAME).so
HEADER := include/rptadvradio/rptadvradio.h
RUST_SOURCES := $(wildcard src/*.rs)
PC_TEMPLATE := rptadvradio.pc.in
PC_FILE := build/rptadvradio.pc
C_SMOKE_SOURCE := tests/descriptor_smoke.c
C_SMOKE_BINARY := build/descriptor-smoke
DEBIAN_VERSION = $(shell dpkg-parsechangelog -S Version)
DEBIAN_ARCH = $(shell dpkg-architecture -qDEB_HOST_ARCH)
DEBIAN_MULTIARCH = $(shell dpkg-architecture -qDEB_HOST_MULTIARCH)
DEBIAN_SOURCE_PARENT = build/debian-source
DEBIAN_OUTPUT_DIR = $(abspath $(DEBIAN_SOURCE_PARENT))
DEBIAN_RUNTIME_DEB = $(DEBIAN_OUTPUT_DIR)/librptadvradio$(SOVERSION)_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_DEV_DEB = $(DEBIAN_OUTPUT_DIR)/librptadvradio-dev_$(DEBIAN_VERSION)_$(DEBIAN_ARCH).deb
DEBIAN_STAGE = build/debian-package-stage
COVERAGE_TOOLCHAIN ?= nightly-2025-02-20
COVERAGE_DIR = build/coverage
COVERAGE_TARGET_DIR = build/llvm-cov-target
COVERAGE_JSON = $(COVERAGE_DIR)/coverage.json
COVERAGE_PRODUCTION_ROOT = $(CURDIR)/src
QUALITY_BASE_IMAGE ?= ghcr.io/cpeter1207/rpt-advanced-quality-debian13:latest
QUALITY_IMAGE ?= $(PACKAGE)-quality:local
QUALITY_LAUNCHER = tools/run-in-quality-container.sh

# Rust emits the ABI-major SONAME while this Makefile creates conventional
# versioned and linker symlinks for Debian and pkg-config consumers.
SONAME_RUSTFLAGS = $(RUSTFLAGS) -C link-arg=-Wl,-soname,$(LIBRARY_BASENAME).so.$(SOVERSION)

.PHONY: all quality lint static-analysis docs rustdoc test coverage install install-check \
	debian-package-check dist distcheck platform-verify ci quality-image container-coverage clean FORCE

all: $(LIBRARY_VERSIONED) $(LIBRARY_SONAME) $(LIBRARY_LINK)

build:
	mkdir -p $@

$(TARGET_LIBRARY): Makefile Cargo.toml Cargo.lock $(RUST_SOURCES)
	RUSTFLAGS="$(SONAME_RUSTFLAGS)" $(CARGO) build --release --locked

$(LIBRARY_VERSIONED): $(TARGET_LIBRARY) | build
	cp $(TARGET_LIBRARY) $@
	$(READELF) -d $@ | grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	! $(READELF) -d $@ | grep -E 'libstd-|\((RPATH|RUNPATH)\)'

$(LIBRARY_SONAME): $(LIBRARY_VERSIONED)
	ln -sf $(notdir $<) $@

$(LIBRARY_LINK): $(LIBRARY_SONAME)
	ln -sf $(notdir $<) $@

$(PC_FILE): $(PC_TEMPLATE) FORCE | build
	sed -e 's|@PREFIX@|$(PREFIX)|' -e 's|@LIBDIR@|$(LIBDIR)|' \
		-e 's|@VERSION@|$(PACKAGE_VERSION)|' -e 's|@ABI_VERSION@|$(SOVERSION)|' $< > $@

quality: lint static-analysis docs

lint:
	$(CARGO_FMT) --check
	$(SHELLCHECK) tools/run-in-quality-container.sh

static-analysis:
	$(CARGO_CLIPPY) --all-targets --all-features -- -D warnings
	$(CPPCHECK) --force --enable=warning,style,performance,portability \
		--error-exitcode=1 --std=c11 -Iinclude $(C_SMOKE_SOURCE)

docs: rustdoc | build
	$(DOXYGEN) Doxyfile
	test ! -s build/doxygen-warnings.log

rustdoc:
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --no-deps --locked --document-private-items

test:
	$(PYTHON) -m unittest discover -s tests -p 'test_*.py'
	$(CARGO) test --all-targets --locked
	$(MAKE) $(C_SMOKE_BINARY)
	./$(C_SMOKE_BINARY)

$(C_SMOKE_BINARY): $(C_SMOKE_SOURCE) $(HEADER) $(LIBRARY_LINK) | build
	$(CC) -std=c11 -Wall -Wextra -Werror -Iinclude $< -Lbuild \
		-l$(CRATE) -Wl,-rpath,'$$ORIGIN' -o $@

# Native Debian 13 amd64 only: audit the JSON rather than trusting a human
# summary, and exclude the in-tree Rust test module from production coverage.
coverage:
	rm -rf $(COVERAGE_DIR) $(COVERAGE_TARGET_DIR)
	mkdir -p $(COVERAGE_DIR)
	RUSTUP_TOOLCHAIN=$(COVERAGE_TOOLCHAIN) CARGO_LLVM_COV_TARGET_DIR=$(abspath $(COVERAGE_TARGET_DIR)) \
		$(CARGO_LLVM_COV) --all-targets --locked --branch --json --output-path $(COVERAGE_JSON)
	$(PYTHON) -c 'import json, os, sys; report=json.load(open(sys.argv[1], encoding="utf-8")); root=os.path.realpath(sys.argv[2]); files={}; [files.setdefault(path, entry["summary"]) for datum in report.get("data", []) for entry in datum.get("files", []) for path in (os.path.realpath(entry["filename"]),) if os.path.commonpath((root,path)) == root and "{}tests{}".format(os.path.sep,os.path.sep) not in path]; failures=[(path,metric,summary.get(metric,{})) for path,summary in sorted(files.items()) for metric in ("lines","branches") if not isinstance(summary.get(metric),dict) or summary[metric].get("covered") != summary[metric].get("count")]; print("verified production coverage for {} source files".format(len(files))); [print("{}: {} {}/{}".format(path,metric,values.get("covered","missing"),values.get("count","missing")),file=sys.stderr) for path,metric,values in failures]; raise SystemExit(1 if not files or failures else 0)' $(COVERAGE_JSON) $(COVERAGE_PRODUCTION_ROOT)

quality-image:
	docker image pull $(QUALITY_BASE_IMAGE)
	docker build --pull --build-arg BASE_IMAGE=$(QUALITY_BASE_IMAGE) --tag $(QUALITY_IMAGE) \
		--file containers/quality.Dockerfile containers

container-coverage: quality-image
	RPTADV_CONTAINER_PULL=0 sh $(QUALITY_LAUNCHER) $(QUALITY_IMAGE) $(MAKE) coverage

install: all $(PC_FILE)
	install -d $(DESTDIR)$(LIBDIR) $(DESTDIR)$(PREFIX)/include/rptadvradio \
		$(DESTDIR)$(LIBDIR)/pkgconfig
	install -m 0755 $(LIBRARY_VERSIONED) $(DESTDIR)$(LIBDIR)/
	ln -sf $(notdir $(LIBRARY_VERSIONED)) $(DESTDIR)$(LIBDIR)/$(notdir $(LIBRARY_SONAME))
	ln -sf $(notdir $(LIBRARY_SONAME)) $(DESTDIR)$(LIBDIR)/$(notdir $(LIBRARY_LINK))
	install -m 0644 $(HEADER) $(DESTDIR)$(PREFIX)/include/rptadvradio/
	install -m 0644 $(PC_FILE) $(DESTDIR)$(LIBDIR)/pkgconfig/

install-check: all
	rm -rf build/stage
	$(MAKE) DESTDIR=$(CURDIR)/build/stage PREFIX=/usr LIBDIR=/usr/lib install
	test -f build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED))
	test -L build/stage/usr/lib/$(notdir $(LIBRARY_SONAME))
	test -L build/stage/usr/lib/$(notdir $(LIBRARY_LINK))
	test ! -e build/stage/usr/lib/$(LIBRARY_BASENAME).a
	$(READELF) -d build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED)) | grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	! $(READELF) -d build/stage/usr/lib/$(notdir $(LIBRARY_VERSIONED)) | grep -E 'res_usbradio.so|libportaudio.so|libasound.so|libhamlib.so|libavfilter.so'
	test -f build/stage/usr/include/rptadvradio/$(notdir $(HEADER))
	test -f build/stage/usr/lib/pkgconfig/rptadvradio.pc
	test "$$(PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig $(PKG_CONFIG) --variable=abi_version rptadvradio)" = '$(SOVERSION)'
	PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig \
		PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/build/stage $(CC) -std=c11 -Wall -Wextra -Werror \
		$(C_SMOKE_SOURCE) $$(PKG_CONFIG_PATH=$(CURDIR)/build/stage/usr/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/build/stage $(PKG_CONFIG) --cflags --libs rptadvradio) \
		-Wl,-rpath,$(CURDIR)/build/stage/usr/lib -o build/stage/descriptor-smoke
	LD_LIBRARY_PATH="$(CURDIR)/build/stage/usr/lib:$$LD_LIBRARY_PATH" build/stage/descriptor-smoke

debian-package-check: dist
	rm -rf $(DEBIAN_SOURCE_PARENT) $(DEBIAN_STAGE)
	mkdir -p $(DEBIAN_SOURCE_PARENT)
	build_root=$$(mktemp -d); \
	trap 'rm -rf "$$build_root"' EXIT; \
	tar -C "$$build_root" -xzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz; \
	source_dir="$$build_root/$(PACKAGE)-$(PACKAGE_VERSION)"; \
	chmod 0644 "$$source_dir"/debian/changelog "$$source_dir"/debian/control \
		"$$source_dir"/debian/copyright "$$source_dir"/debian/*.docs \
		"$$source_dir"/debian/*.install "$$source_dir"/debian/source/*; \
	chmod 0755 "$$source_dir"/debian/rules; \
	cd "$$source_dir" && dpkg-buildpackage -us -uc -b; \
	cp "$$build_root"/*.deb "$(DEBIAN_OUTPUT_DIR)/"
	test -f "$(DEBIAN_RUNTIME_DEB)"
	test -f "$(DEBIAN_DEV_DEB)"
	rm -rf $(DEBIAN_STAGE)
	mkdir -p $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_RUNTIME_DEB)" $(DEBIAN_STAGE)
	dpkg-deb --extract "$(DEBIAN_DEV_DEB)" $(DEBIAN_STAGE)
	test ! -e "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).a"
	test -L "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so"
	$(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so.$(SOVERSION)" | grep -F '$(LIBRARY_BASENAME).so.$(SOVERSION)'
	! $(READELF) -d "$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/$(LIBRARY_BASENAME).so.$(SOVERSION)" | grep -E 'res_usbradio.so|libportaudio.so|libasound.so|libhamlib.so|libavfilter.so'
	PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig \
		PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(CC) -std=c11 -Wall -Wextra -Werror \
		$(C_SMOKE_SOURCE) $$(PKG_CONFIG_PATH=$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH)/pkgconfig PKG_CONFIG_SYSROOT_DIR=$(CURDIR)/$(DEBIAN_STAGE) $(PKG_CONFIG) --cflags --libs rptadvradio) \
		-Wl,-rpath,$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH) -o $(DEBIAN_STAGE)/descriptor-smoke
	LD_LIBRARY_PATH="$(CURDIR)/$(DEBIAN_STAGE)/usr/lib/$(DEBIAN_MULTIARCH):$$LD_LIBRARY_PATH" $(DEBIAN_STAGE)/descriptor-smoke

dist: | build
	rm -rf build/dist
	mkdir -p build/dist/$(PACKAGE)-$(PACKAGE_VERSION)
	tar --exclude=.git --exclude=.work --exclude=build --exclude=target --exclude=__pycache__ \
		--exclude=debian/.debhelper --exclude=debian/debhelper-build-stamp \
		--exclude=debian/files --exclude=debian/tmp \
		--exclude=debian/librptadvradio3 --exclude=debian/librptadvradio-dev \
		--exclude='debian/*.substvars' --exclude='debian/*.debhelper.log' \
		--transform='s|^|$(PACKAGE)-$(PACKAGE_VERSION)/|' -czf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz \
		.dockerignore AGENTS.md Cargo.lock Cargo.toml COPYING Doxyfile Makefile QUALITY.md README.md \
		rptadvradio.pc.in rust-toolchain.toml containers debian include src tests tools

distcheck: dist
	! tar -tzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz | \
		grep -E '/debian/(\.debhelper/|debhelper-build-stamp$$|files$$|tmp/|librptadvradio([0-9]+|-dev)/|.*\.(substvars|debhelper\.log)$$)'
	rm -rf build/dist-unpacked
	mkdir -p build/dist-unpacked
	tar -C build/dist-unpacked -xzf build/$(PACKAGE)-$(PACKAGE_VERSION).tar.gz
	$(MAKE) -C build/dist-unpacked/$(PACKAGE)-$(PACKAGE_VERSION) install-check

platform-verify: test install-check debian-package-check distcheck

ci: quality platform-verify

clean:
	rm -rf build target

FORCE:
