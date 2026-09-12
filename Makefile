.PHONY: build test package clean
build:
	cargo build --release --locked
test:
	cargo test --locked
package: build
	./packaging/build-deb.sh
clean:
	cargo clean
	rm -rf dist
