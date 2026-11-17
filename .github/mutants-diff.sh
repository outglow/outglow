#!/usr/bin/env bash
# The mutants in the lines a diff changes (workflow mutants.yml, job diff):
#   .github/mutants-diff.sh <diff> <output dir> [jobs]
# bulb-core apart: its behaviour is tested in bulb-sim (the simulated bulb),
# so its mutants run bulb-sim's tests too, with a timeout of their own (the
# test timeout cargo-mutants sets comes from bulb-core's tests alone).
# crates/bulb-tc32 is a workspace of its own (config crates/bulb-tc32/.cargo/
# mutants.toml): its part of the diff, with paths relative to it, goes to a
# third pass. More than LIMIT mutants in a pass (a sweeping change) and an
# even sample of LIMIT runs (round-robin --shard 0/K): the weekly run tests
# them all.
set -euo pipefail

diff=$1
out=$2
jobs=${3:-2}
limit=${LIMIT:-400}

mkdir -p "$out"
tc32_diff="$out/bulb-tc32.diff"
awk '/^diff --git /{keep = index($0, " a/crates/bulb-tc32/") > 0} keep' "$diff" \
  | sed -E 's#^(---|\+\+\+) (a|b)/crates/bulb-tc32/#\1 \2/#; s#^diff --git a/crates/bulb-tc32/(.*) b/crates/bulb-tc32/#diff --git a/\1 b/#' > "$tc32_diff"

passes=(
  "rest|$diff|--exclude crates/bulb-core/**"
  "bulb-core|$diff|-p bulb-core --test-package bulb-core,bulb-sim --timeout 120"
  "bulb-tc32|$tc32_diff|--manifest-path crates/bulb-tc32/Cargo.toml"
)

for pass in "${passes[@]}"; do
  name=${pass%%|*}
  rest=${pass#*|}
  diff=${rest%%|*}
  read -r -a args <<< "${rest#*|}"
  if [ ! -s "$diff" ]; then
    echo "$name: no changes"
    continue
  fi
  n=$(cargo mutants --list --in-diff "$diff" "${args[@]}" | wc -l)
  if [ "$n" -eq 0 ]; then
    echo "$name: no mutants in the changed lines"
    continue
  fi
  sample=()
  if [ "$n" -gt "$limit" ]; then
    k=$(( (n + limit - 1) / limit ))
    sample=(--shard "0/$k" --sharding round-robin)
    echo "$name: $n mutants in the changed lines, testing every ${k}th" | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
  else
    echo "$name: $n mutants in the changed lines"
  fi
  mkdir -p "$out/$name"
  # (exit 2 = some missed: the report decides)
  cargo mutants --in-diff "$diff" "${args[@]}" "${sample[@]}" --jobs "$jobs" --output "$out/$name" || true
done
