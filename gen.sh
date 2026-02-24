#!/bin/bash

PMM=$1

# ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=1.0,4000,5 --src-token=wsol --dst-token=usdc --jit-accounts=false
# ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=1.0,4000,5 --src-token=wsol --dst-token=usdc --jit-accounts=false --spoof=dflow
# ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=1.0,4000,5 --src-token=wsol --dst-token=usdc --jit-accounts=false --spoof=titan
# ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=1.0,4000,5 --src-token=wsol --dst-token=usdc --jit-accounts=false --spoof=okxlabs
# ./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=1.0,4000,5 --src-token=wsol --dst-token=usdc --jit-accounts=false --spoof=jupiter
# ./target/release/pmm-sim benchmark --call-type=direct --pmms=$PMM --range=1.0,4000,5 --jit-accounts=false

./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=100,40000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false
./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=100,40000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false --spoof=dflow
./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=100,40000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false --spoof=titan
./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=100,40000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false --spoof=okxlabs
./target/release/pmm-sim benchmark --call-type=cpi --pmms=$PMM --range=100,40000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false --spoof=jupiter
./target/release/pmm-sim benchmark --call-type=direct --pmms=$PMM --range=100,4000,50 --src-token=usdt --dst-token=usdc --jit-accounts=false
