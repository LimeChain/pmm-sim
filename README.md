# pmm-sim

Simulation environment for Solana's Proprietary AMMs. The setup relies on a custom router that facilitates the interaction between different AMM protocols within the environment.

Supported AMMs:

- [x] Humidifi
- [x] SolfiV2
- [x] ObricV2
- [x] Zerofi
- [ ] TesseraV
- [ ] AlphaQ
- [ ] Goonfi
- [ ] Aquifer
- [ ] Bisonfi

The swaps can be done either with the local static accounts that can be found at [cfg/accounts](./cfg/accounts) or with the current live accounts (by fetching them on-the-go). By default all swaps are done with the live accounts.

## Examples

```
# Swaps 150,000 USDC to WSOL using Humidifi and SolfiV2 - 75000 USDC per Prop AMM

cargo r -- single --dexes=humidifi,solfi-v2 --weights=50,50 --amount-in=150000 --src-token=USDC --dst-token=WSOL
```

```
# Swaps 10,000 USDC to USDT using ObricV2

cargo r -- single --amount-in=10000 --dexes=obric-v2 --weights=100 --src-token=USDC --dst-token=USDT
```

```
# Locally sync the current live accounts for all supported AMMs

cargo r -- fetch-accounts
```
