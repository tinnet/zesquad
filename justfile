# zesquad dev tasks. `just` lists them.

# Battle.net credentials for `run`, `api` and `network-test` come from .env (see .env.example).
set dotenv-load

[private]
default:
    @{{ just_executable() }} --list --unsorted

# Install the tools from mise.toml and the git hooks
setup:
    mise install
    lefthook install

# Run the tests (no network; Battle.net is faked)
test *args:
    cargo test --workspace {{ args }}

# Run the tests that call the real Battle.net API; needs .env and ZESQUAD_TEST_CHARACTER=Name-Surname
network-test:
    cargo test --workspace -- --ignored

# Clippy with every pedantic warning as an error, and the Mermaid diagrams in docs/
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    for f in docs/*.md; do merman-cli lint "$f" >/dev/null || exit 1; done

# Serve the app on http://127.0.0.1:8080 with a database in data/
run *args:
    cargo run --quiet -- serve {{ args }}

# GET a Battle.net API path with the app token and print the response, e.g. `just api /data/wow/region/index dynamic-forever-eu`
api path namespace="profile-forever-eu":
    cargo run --quiet -- api '{{ path }}' --namespace '{{ namespace }}'

# Format the code and this justfile
fmt:
    cargo fmt --all
    {{ just_executable() }} --fmt

# Fail if anything is not formatted
fmt-check:
    cargo fmt --all --check
    {{ just_executable() }} --fmt --check

# What CI runs
ci: fmt-check lint test

# Release build for this machine
build:
    cargo build --workspace --release

# Build the Linux release binary for this machine's arch and the container image (needs cargo-zigbuild)
[unix]
docker:
    #!/usr/bin/env sh
    set -eu
    case "$(uname -m)" in arm64|aarch64) arch=arm64 target=aarch64-unknown-linux-musl ;; *) arch=amd64 target=x86_64-unknown-linux-musl ;; esac
    rustup target add "$target"
    cargo zigbuild --release --target "$target" -p zesquad
    mkdir -p "dist/linux-$arch"
    cp "target/$target/release/zesquad" "dist/linux-$arch/zesquad"
    docker build --platform "linux/$arch" -t zesquad:dev .

# Show what the next release's notes would say
changelog:
    git cliff --unreleased --strip header

# Release from main (today's date, or e.g. `just release 2026.11.10.1`): commit CHANGELOG.md, tag, push
[unix]
release version=`date +%Y.%m.%d`:
    git diff --quiet HEAD && git fetch -q origin main && test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" || { echo "release from a clean main that matches origin/main" >&2; exit 1; }
    git cliff --tag "v{{ version }}" --output CHANGELOG.md
    git add CHANGELOG.md
    git diff --cached --quiet || git commit -q -m "chore(release): {{ version }}"
    git push origin HEAD:main
    git tag -s "v{{ version }}" -m "zesquad {{ version }}"
    git push origin "v{{ version }}"
