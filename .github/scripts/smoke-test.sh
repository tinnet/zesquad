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

# Static or static-pie: no program interpreter, so it starts on distroless/static, which has no
# dynamic loader. Read the ELF headers rather than ldd's wording, which varies by glibc version
# and says "not a dynamic executable" for a musl-linked binary too.
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

# A free port, so the health check can't reach some other server on a fixed one.
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
"$bin" serve --listen "127.0.0.1:$port" --database "${RUNNER_TEMP:-/tmp}/smoke-$port.db" &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT

for _ in $(seq 20); do
  # Ours must still be running, not just something answering on the port.
  kill -0 "$pid" 2>/dev/null || { echo "zesquad serve exited" >&2; exit 1; }
  "$bin" healthcheck --listen "127.0.0.1:$port" && exit 0
  sleep 0.5
done
echo "zesquad serve not healthy after 10s" >&2
exit 1
