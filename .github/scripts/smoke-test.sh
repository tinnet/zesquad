#!/usr/bin/env bash
# Smoke test for a Linux release binary, run by ci.yml and release.yml on the runner that built it.
#
#   smoke-test.sh <binary> [version]
#
# Fails unless the binary is static, prints `zesquad <version>` (when a version is given), and
# serves /healthz.
set -euo pipefail

bin=$1
version=${2:-}

# Static or static-pie: no PT_INTERP, so it runs on distroless/static (no dynamic loader).
headers=$(readelf --program-headers --wide "$bin")
if grep -q INTERP <<<"$headers"; then
  echo "$bin is dynamically linked:" >&2
  grep -A1 INTERP <<<"$headers" >&2
  exit 1
fi

if [ -n "$version" ]; then
  out=$("$bin" --version)
  echo "$out"
  [ "$out" = "zesquad $version" ]
fi

# Port 0, then the address serve logs it bound: the health check reaches this server, no other.
dir=$(mktemp -d)
"$bin" serve --listen 127.0.0.1:0 --database "$dir/smoke.db" --log-json >"$dir/log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; rm -rf "$dir"' EXIT

tries=100
for _ in $(seq "$tries"); do
  kill -0 "$pid" 2>/dev/null || { echo "zesquad serve exited:" >&2; cat "$dir/log" >&2; exit 1; }
  addr=$(grep -o 'listening on http://[^"]*' "$dir/log" | cut -d/ -f3 || true)
  [ -n "$addr" ] && "$bin" healthcheck --listen "$addr" && exit 0
  sleep 0.1
done
echo "zesquad serve not healthy after $tries tries:" >&2
cat "$dir/log" >&2
exit 1
