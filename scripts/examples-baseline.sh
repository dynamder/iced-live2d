#!/usr/bin/env bash
# Renders every example's scene headlessly and prints one sha256 per recipe.
#
# The iced examples need a window, so this reproduces their scene — same model,
# texture budget, mask budget, fill and first motion — through `offscreen`, which
# drives the same `Session` and render path. A refactor must not change these
# hashes. See `docs/examples-baseline.md`.
#
# usage: scripts/examples-baseline.sh <model-dir> <out-dir>
set -euo pipefail

models=${1:-.refs/models}
out=${2:-target/examples-baseline}
offscreen=target/release/examples/offscreen

mkdir -p "$out"

render() {
    local name=$1 model=$2 scale=$3 mask=$4 fill=$5 motion=$6 size=$7 steps=$8
    local -a vars=(LIVE2D_TEXTURE_SCALE="$scale" LIVE2D_MASK="$mask" LIVE2D_FILL="$fill")

    if [ -n "$motion" ]; then
        vars+=(LIVE2D_MOTION="$motion")
    fi

    env "${vars[@]}" "$offscreen" "$model" "$out/$name.png" "$size" "$steps" >/dev/null

    printf '%s  %s\n' "$(sha256sum "$out/$name.png" | cut -d' ' -f1)" "$name"
}

render app       "$models/Hiyori/Hiyori.model3.json" 0.5  1024 0.925 0,0 640 30
render minimal   "$models/Hiyori/Hiyori.model3.json" 0.5  1024 0.925 0,0 640 30
render interact  "$models/Hiyori/Hiyori.model3.json" 0.5  1024 0.925 0,0 640 30
render policy    "$models/Hiyori/Hiyori.model3.json" 0.5  1024 0.925 0,0 640 30
render typed     "$models/Wanko/Wanko.model3.json"   0.5  1024 0.925 0,0 640 30
render offscreen "$models/Hiyori/Hiyori.model3.json" 1.0  1024 0.925 ""  900 30
