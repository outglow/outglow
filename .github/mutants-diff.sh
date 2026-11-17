#!/usr/bin/env bash
# The mutants in the lines a diff changes (workflow mutants.yml, job diff):
#   .github/mutants-diff.sh <diff> <output dir> [jobs]
# bulb-core apart: its behaviour is tested in bulb-sim (the simulated bulb),
# so its mutants run bulb-sim's tests too, with a timeout of their own (the
# test timeout cargo-mutants sets comes from bulb-core's tests alone). More than LIMIT mutants (a sweeping
# change) and an even sample of LIMIT runs (round-robin --shard 0/K): the
# weekly run tests them all.
set -euo pipefail

diff=$1
out=$2
jobs=${3:-2}
limit=${LIMIT:-400}

passes=(
  "rest|--exclude crates/bulb-core/**"
  "bulb-core|-p bulb-core --test-package bulb-core,bulb-sim --timeout 120"
)

for pass in "${passes[@]}"; do
  name=${pass%%|*}
  read -r -a args <<< "${pass#*|}"
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
