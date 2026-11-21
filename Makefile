.PHONY: check
check:
	nix fmt -- --ci
	cargo clippy --all-targets -- -D warnings
	lake -d lean build --wfail
	lake -d audit build --wfail
	git diff --exit-code -- .leanfran lean
	test -z "$$(git ls-files --others --exclude-standard -- .leanfran lean)"
	cargo test

.PHONY: format
format:
	nix fmt
