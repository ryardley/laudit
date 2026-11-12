.PHONY: all test test-rust test-nvim fmt fmt-check clippy check clean

all: test

test: test-rust test-nvim

test-rust:
	cargo test --workspace

test-nvim:
	bash tests/nvim/run.sh

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

check: fmt-check clippy test

clean:
	cargo clean
