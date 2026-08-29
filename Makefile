.PHONY: build-client check run ui-editor wz-editor

build-client:
	cd crates/oozems-client && NO_COLOR=false trunk build --release --dist ../oozems-server/public

check:
	cargo +nightly fmt --all -- --check
	cargo check --workspace --all-targets
	cargo test --workspace
	cargo check --package oozems-client --target wasm32-unknown-unknown

run: build-client
	cargo run --release --package oozems-server --bin oozems-server

ui-editor:
	cargo run --release --package oozems-ui-editor

wz-editor:
	cargo run --release --package oozems-wz-editor
