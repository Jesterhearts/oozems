.PHONY: build-client check run

build-client:
	cd crates/oozems-client && NO_COLOR=false trunk build --release --dist ../oozems-server/public

check:
	cargo +nightly fmt --all -- --check
	cargo check --workspace --all-targets
	cargo test --workspace
	cargo check --package oozems-client --target wasm32-unknown-unknown

run: build-client
	cargo run --package oozems-server --bin oozems-server
