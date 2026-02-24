# pmm-sim

Simulation & Benchmark environment for Solana's Proprietary AMMs. The setup relies on [Litesvm](https://crates.io/crates/litesvm) for local, consistent and expedited execution.

Supported Prop AMMs:

- [x] HumidiFi (swap v1/v2/v3)
- [x] SolFiV2
- [x] ObricV2
- [x] ZeroFi
- [x] TesseraV
- [x] GoonFi
- [x] BisonFi

Swaps and benchmarks can be done through direct offchain calls, or alternatively through CPI calls from an onchain router - [magnus-router](https://github.com/LimeChain/magnus/tree/master/crates/router). Additionally, Router CPI calls can be spoofed as one of the following aggregators: Jupiter, Okx, Dflow or Titan. Some Prop AMMs tend to provide different — preferential — rates for whitelisted set of addresses. Depending on who's the source of CPI, you might get different rates.

The swaps and benchmarks can be done with the local static accounts/programs stored at [./cfg/](./cfg/) or with the current live ones by fetching them on-the-go. By default all swaps & benchmark simulations are done with real-time accounts/programs. The markets for each Prop AMM are specified in [./cfg/setup.toml](./cfg/setup.toml).

![all](./assets/all_rate.png)
_Figure 1: Exchange rates for WSOL -> USDC at discrete slot `401101387`, with spoofed CPI call as Jupiter._

Possible modes of execution include:

- **single** - Run a single swap route across one or more Prop AMMs with specified weights. The route can go through an arbitrary combination of Prop AMMs.
- **multi** - Execute swaps across nested Prop AMM routes. Each inner list represents a single route, each route possibly going through multiple Prop AMMs.
- **direct** - Execute a direct, non-cpi swap with some of the Prop AMMs.
- **benchmark** - Benchmark swaps for any of the implemented Prop AMMs by specifying, optionally, spoofing, the src/dst tokens and range size. Benchmark data can be visualised with [plot.py](./scripts/plot.py).
- **fetch-accounts** - Fetch accounts for specified PMMs via RPC and save them locally (presumably for later usage).
- **fetch-programs** - Fetch programs for specified PMMs via RPC and save them locally (presumably for later usage).

Example benchmarked exchange rate & CU usage:

| Prop AMM         | Exchange Rate                           | Compute Units                         |
| ---------------- | --------------------------------------- | ------------------------------------- |
| HumidiFi         | ![](./assets/humidifi_rate.png)         | ![](./assets/humidifi_cu.png)         |
| HumidiFi Swap V2 | ![](./assets/humidifi-swap-v2_rate.png) | ![](./assets/humidifi-swap-v2_cu.png) |
| HumidiFi Swap V3 | ![](./assets/humidifi-swap-v3_rate.png) | ![](./assets/humidifi-swap-v3_cu.png) |
| SolFi V2         | ![](./assets/solfi-v2_rate.png)         | ![](./assets/solfi-v2_cu.png)         |
| BisonFi          | ![](./assets/bisonfi_rate.png)          | ![](./assets/bisonfi_cu.png)          |
| Tessera          | ![](./assets/tessera_rate.png)          | ![](./assets/tessera_cu.png)          |
| Obric V2         | ![](./assets/obric-v2_rate.png)         | ![](./assets/obric-v2_cu.png)         |
| GoonFi           | ![](./assets/goonfi_rate.png)           | ![](./assets/goonfi_cu.png)           |

---

To explicitly specify the market the operation should execute against, for a particular prop AMM, suffix the Prop AMM's name with a substring of the address of the market. If no market is specified explicitly, we'll default to the first one defined in [./cfg/setup.toml](./cfg/setup.toml).

## Examples

Build the project

```
cargo build --release
```

### Single-route swaps

##### Swap 15K USDC for WSOL using HumidiFi's swap-v1.

```
./target/release/pmm-sim single --amount-in=15000 --pmms=humidifi --weights=100 \
  --src-token=USDC --dst-token=WSOL
```

#### Swap 15,345 USDT for WSOL using GoonFi, spoofed as Jupiter.

```
./target/release/pmm-sim single --amount-in=15345 --pmms=goonfi --weights=100 --src-token=usdc --dst-token=wsol --jit-accounts=false --spoof=jupiter
```

##### Swap 69K USDC for WSOL using HumidiFi (swap-v2 instruction on the [Fk market](https://solscan.io/account/FksffEqnBRixYGR791Qw2MgdU7zNCpHVFYBL4Fa4qVuH)) and BisonFi, in one route, split 25%,75% accordingly.

```
./target/release/pmm-sim single --amount-in=69000 --pmms=humidifi-swap-v2_Fk,bisonfi --weights=25,75 \
  --src-token=USDC --dst-token=WSOL
```

##### Swap 375 WSOL for USDC using Tessera and SolFiV2's [65Z market](https://solscan.io/account/65ZHSArs5XxPseKQbB1B4r16vDxMWnCxHMzogDAqiDUc), in one route, split evenly - 187,5 WSOL per Prop AMM, spoofed as DFlow.

```
./target/release/pmm-sim single --spoof=dflow --amount-in=375 --pmms=tessera,solfi-v2_65Z \
  --weights=50,50 --src-token=WSOL --dst-token=USDC
```

##### Swap 100 WSOL for USDC using SolFiV2, HumidiFi, and Tessera, in one route, split 33,33,34 WSOL per Prop AMM.

```
./target/release/pmm-sim single --amount-in=100 --pmms=solfi-v2,humidifi,tessera \
  --weights=33,33,34 \
  --src-token=WSOL --dst-token=USDC \
  --jit-accounts=false --jit-programs=false
```

##### Swaps 10,000 USDC for USDT using ObricV2.

```
./target/release/pmm-sim single --amount-in=10000 --pmms=obric-v2 --weights=100 \
  --src-token=USDC --dst-token=USDT
```

### Multi-route swaps

##### Swap 103 WSOL for USDC in a multi-route swap, 100 WSOL via HumidiFi and SolFiV2 (split 92%/8%) in one route, and 3 WSOL via Tessera in another route.

```
./target/release/pmm-sim multi --amount-in=100,3 --pmms="[[humidifi,solfi-v2],[tessera]]" \
  --weights="[[92,8],[100]]"
```

##### Execute two routes, the first swapping 150,000 USDC for WSOL using HumidiFi and SolFiV2 (split 25%/75%), the second swapping 1000 USDC for WSOL using BisonFi.

```
RUST_LOG=debug ./target/release/pmm-sim multi --amount-in=150000,1000 --pmms="[[humidifi,solfi-v2],[bisonfi]]" --weights="[[25,75],[100]]" --src-token=USDC --dst-token=WSOL
```

### Direct calls

##### Execute a direct offchain call towards HumidiFi's swap-v3 instruction, swapping 350 WSOL for USDC.

```
RUST_LOG=debug ./target/release/pmm-sim direct --pmm=humidifi-swap-v3 --amount-in=350 --src-token=WSOL --dst-token=USDC
```

##### Execute a direct offchain call towards BisonFi's [FC9 market](https://solscan.io/account/FC9pWtfdtbyGZ5WHTLneoMSUx6jmTDgqKaxDcm2trsND), swapping 35K USDT for WSOL.

```
./target/release/pmm-sim direct --pmm=bisonfi_FC9 --amount-in=35000 --src-token=USDT --dst-token=WSOL
```

### Benchmark swaps

##### Benchmark swaps on HumidiFi,Tessera,SolFiV2 and GoonFi, from 1 to 4000 WSOL to USDC, in increments of 1 WSOL, using the accounts & programs from [./cfg](./cfg). The results are saved at [./datasets](./datasets).

```
./target/release/pmm-sim benchmark --pmms=humidifi,tessera,solfi-v2,goonfi \
  --range=1.0,4000.0,1.0 --src-token=wsol --dst-token=usdc \
  --jit-accounts=false --jit-programs=false
```

##### Benchmark swaps on Tessera and SolFiV2, from 1 to 250 WSOL, in increments of 0.01 WSOL. The results are saved at [./datasets](./datasets).

```
./target/release/pmm-sim benchmark --pmms=tessera,solfi-v2 \
  --range=1.0,250.0,0.01 --src-token=wsol --dst-token=usdc
```

##### Benchmark swaps (USDC->WSOL) on HumidiFi and SolFiV2, from 10K to 100K USDC, in increments of 100 USDC. The results are saved at [./datasets](./datasets).

```
./target/release/pmm-sim benchmark --pmms=humidifi,solfi-v2 \
  --range=10000,100000,100 --src-token=usdc --dst-token=wsol
```

Generated benchmark data can be plotted through [./scripts/plot.py](./scripts/plot.py), like so:

##### Plot the datasets for slot `389141713`.

```
./scripts/plot.py ./datasets/389141713*
```

### Fetch current accounts

##### Locally sync the current (live) accounts for BisonFi and HumidiFi's swap-v3.

```
./target/release/pmm-sim fetch-accounts --pmms=bisonfi,humidifi-swap-v3
```

##### Locally sync the current (live) accounts for Tessera and SolFiV2.

```
./target/release/pmm-sim fetch-accounts --pmms=tessera,solfi-v2
```

### Fetch current programs

##### Locally sync the current (live) programs for all supported Prop AMMs.

```
./target/release/pmm-sim fetch-programs
```

##### Locally sync the current (live) programs for BisonFi and Tessera.

```
./target/release/pmm-sim fetch-programs --pmms=bisonfi,tessera
```

---

All datasets are saved as `parquet` and available at [datasets](./datasets). To peek at the data through cli:

```sh
duckdb -csv \
    -c "SELECT * FROM 'datasets/datasets/401101387_magnus_bisonfi_51FQwjrvo8J8zXUaKyAznJ5NYpoiTCuqAqCu3HAMB9NZ_20260218-205826.parquet'" \
    | column -t -s ,
```

---

Accounts are by default loaded (saved) from (at) [cfg/accounts](./cfg/accounts). Tweaking the source/destination is possible via `--accounts-path` or `ACCOUNTS_PATH` env variable.

Programs are by default loaded (saved) from (at) [cfg/programs](./cfg/programs). Tweaking the source/destination is possible via `--programs-path` or `PROGRAMS_PATH` env variable.

Datasets are by default loaded (saved) from (at) [datasets](./datasets). Tweaking the source/destination is possible via `--datasets-path` or `DATASETS_PATH` env variable.

The supported tokens are defined in [cfg/setup.toml](./cfg/setup.toml) under `[[tokens]]` entries. The markets are defined in [cfg/setup.toml](./cfg/setup.toml) under the corresponding PMM.

---

Check out the CLI subcommands for additional clues (i.e `pmm-sim single --help`)

```
$ pmm-sim --help
Simulation environment for Solana's Proprietary AMMs.
Simulate swaps and Benchmark performance across *any* of the major Solana Prop AMMs.

Usage: pmm-sim <COMMAND>

Commands:
  direct          Initialize an environment for a single PMM and execute a direct swap.
  router-single   Run a single swap route across one or more Prop AMMs with specified weights.
  router-multi    Execute multiple swap routes across nested Prop AMM routes. Each inner list represents a single route, each route possibly going through multiple Prop AMMs.
  benchmark       Benchmark swaps for any one of the implemented Prop AMMs by specifying, optionally, the accounts, src/dst tokens and step size
  fetch-accounts  Fetch accounts from the specified Pmms via RPC and save them locally (presumably for later usage).
  fetch-programs  Fetch programs from the specified Pmms via RPC and save them locally (presumably for later usage).
  help            Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

---

License: MIT
