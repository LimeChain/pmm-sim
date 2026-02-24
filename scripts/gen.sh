#!/bin/bash

PMM=$1
SPOOFS=("" "dflow" "titan" "okxlabs" "jupiter")

if [ "$PMM" = "obric-v2" ]; then
    RANGE="100,40000,50"
    SRC="usdt"
    DST="usdc"
    DIRECT_RANGE="100,4000,50"
else
    RANGE="1.0,4000,5"
    SRC="wsol"
    DST="usdc"
    DIRECT_RANGE="$RANGE"
fi

for spoof in "${SPOOFS[@]}"; do
    if [ -n "$spoof" ]; then
        ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=$RANGE --src-token=$SRC --dst-token=$DST --jit-accounts=false --spoof=$spoof
    else
        ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=$RANGE --src-token=$SRC --dst-token=$DST --jit-accounts=false
    fi
done

./target/release/pmm-sim benchmark --call-type=direct --pmms=$PMM --range=$DIRECT_RANGE --src-token=$SRC --dst-token=$DST --jit-accounts=false
