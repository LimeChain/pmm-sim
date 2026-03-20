#!/usr/bin/env python3
"""Generate all possible visualisation graphs from PMM benchmark parquet data.

Usage:
    python scripts/plot_all.py datasets/4058968*.parquet --save ./plots
    python scripts/plot_all.py datasets/405895625_*.parquet --save ./plots --no-display
"""

import os
import argparse

import matplotlib.pyplot as plt
import numpy as np
import polars as pl

# ── constants ──────────────────────────────────────────────────────────────────

MARKERS = ["o", "s", "^", "v", "*", "h", "<", ">", "p", "D", "X", "P"]
LINESTYLES = ["--", "-.", ":", "-"]
VIA_TAGS = {"magnus", "jupiter", "okxlabs", "dflow", "titan", "direct"}
LEGEND_KW = dict(fontsize=7, ncol=2, handlelength=1.5, handletextpad=0.4, columnspacing=0.8)

# ── helpers ────────────────────────────────────────────────────────────────────


def extract_via(filepath: str) -> str | None:
    basename = os.path.basename(filepath).removesuffix(".parquet")
    parts = basename.split("_", 2)
    if len(parts) >= 2 and parts[1] in VIA_TAGS:
        return parts[1]
    return None


def load_files(files: list[str]) -> list[tuple[str, str, pl.DataFrame]]:
    """Return list of (filepath, via, dataframe) for non-empty files."""
    result = []
    for f in files:
        df = pl.read_parquet(f)
        if df.is_empty():
            continue
        via = extract_via(f) or "unknown"
        result.append((f, via, df))
    return result


def make_label(df: pl.DataFrame, via: str) -> str:
    pmm = df["pmm"][0]
    return f"{pmm} [{via}]"


def _marker_kw(i: int, n_points: int) -> dict:
    return dict(
        marker=MARKERS[i % len(MARKERS)],
        markersize=4,
        markevery=(i * 7, max(10, n_points // 30)),
        fillstyle="none",
        markeredgewidth=1.0,
    )


def finish(save_path: str | None, block: bool):
    plt.tight_layout()
    if save_path:
        os.makedirs(os.path.dirname(save_path) or ".", exist_ok=True)
        plt.savefig(save_path, dpi=150, bbox_inches="tight")
        print(f"  Saved: {save_path}")
    plt.show(block=block)


def _title_prefix(first_df: pl.DataFrame) -> str:
    return f"{first_df['src_token'][0]} → {first_df['dst_token'][0]} | slot {first_df['slot'][0]}"


# ── 1. Exchange Rate ──────────────────────────────────────────────────────────


def plot_exchange_rate(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (_, via, df) in enumerate(entries):
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        ax.plot(
            df["amount_in"], df["rate"],
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("exchange rate (out/in)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Exchange Rate", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 2. Compute Units ─────────────────────────────────────────────────────────


def plot_compute_units(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (_, via, df) in enumerate(entries):
        ax.plot(
            df["amount_in"], df["compute_units"],
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("compute units", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Compute Units", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 3. Spread (absolute) ─────────────────────────────────────────────────────


def plot_spread(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (f, via, df) in enumerate(entries):
        df = df.filter(pl.col("spread").is_not_nan())
        if df.is_empty():
            continue
        ax.plot(
            df["amount_in"], df["spread"],
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("spread (absolute)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Spread", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 4. Spread (bps) ──────────────────────────────────────────────────────────


def plot_spread_bps(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (f, via, df) in enumerate(entries):
        df = df.filter(pl.col("spread_bps").is_not_nan())
        if df.is_empty():
            continue
        ax.plot(
            df["amount_in"], df["spread_bps"],
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("spread (bps)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Spread (bps)", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 5. Price Impact ──────────────────────────────────────────────────────────


def plot_price_impact(entries, save_path=None, block=False):
    """% degradation of exchange rate relative to the smallest trade."""
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (_, via, df) in enumerate(entries):
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        base_rate = df["rate"][0]
        if base_rate == 0:
            continue
        impact = ((df["rate"] - base_rate) / base_rate * 100).to_list()
        ax.plot(
            df["amount_in"], impact,
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("price impact (%)", fontsize=11)
    ax.axhline(0, color="grey", linewidth=0.8, linestyle="-")
    ax.set_title(f"{_title_prefix(entries[0][2])} | Price Impact (% from base rate)", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 6. Effective Output ─────────────────────────────────────────────────────


def plot_effective_output(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (_, via, df) in enumerate(entries):
        ax.plot(
            df["amount_in"], df["amount_out"],
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    first = entries[0][2]
    ax.set_xlabel(f"amount_in ({first['src_token'][0]})", fontsize=11)
    ax.set_ylabel(f"amount_out ({first['dst_token'][0]})", fontsize=11)
    ax.set_title(f"{_title_prefix(first)} | Effective Output", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 7. Rate Deviation from Direct ────────────────────────────────────────────


def plot_rate_deviation(entries, save_path=None, block=False):
    """For each PMM, show how spoofed rates deviate from the direct rate."""
    # Build direct baselines keyed by pmm
    direct_rates: dict[str, pl.DataFrame] = {}
    for _, via, df in entries:
        if via == "direct":
            pmm = df["pmm"][0]
            direct_rates[pmm] = df.with_columns(
                (pl.col("amount_out") / pl.col("amount_in")).alias("rate")
            )

    if not direct_rates:
        print("  Skipping rate_deviation: no direct entries found")
        return

    _, ax = plt.subplots(figsize=(14, 8))
    idx = 0
    for _, via, df in entries:
        if via == "direct":
            continue
        pmm = df["pmm"][0]
        if pmm not in direct_rates:
            continue
        base = direct_rates[pmm]
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        # Align on amount_in
        merged = df.join(base.select("amount_in", "rate"), on="amount_in", suffix="_direct")
        if merged.is_empty():
            continue
        deviation = ((merged["rate"] - merged["rate_direct"]) / merged["rate_direct"] * 10000).to_list()
        ax.plot(
            merged["amount_in"], deviation,
            linestyle=LINESTYLES[idx % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=f"{pmm} [{via}]", **_marker_kw(idx, len(merged)),
        )
        idx += 1

    ax.axhline(0, color="grey", linewidth=0.8, linestyle="-")
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("rate deviation from direct (bps)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Rate Deviation from Direct", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 8. Spoof Advantage Heatmap ───────────────────────────────────────────────


def plot_spoof_heatmap(entries, save_path=None, block=False):
    """Heatmap: PMM (rows) × via-route (cols) = mean spread_bps."""
    data: dict[tuple[str, str], list[float]] = {}
    for _, via, df in entries:
        df = df.filter(pl.col("spread_bps").is_not_nan())
        if df.is_empty():
            continue
        pmm = df["pmm"][0]
        data.setdefault((pmm, via), []).extend(df["spread_bps"].to_list())

    if not data:
        print("  Skipping spoof_heatmap: no spread_bps data")
        return

    pmms = sorted({k[0] for k in data})
    vias = sorted({k[1] for k in data})

    matrix = np.full((len(pmms), len(vias)), np.nan)
    for (pmm, via), vals in data.items():
        r = pmms.index(pmm)
        c = vias.index(via)
        matrix[r, c] = np.mean(vals)

    fig, ax = plt.subplots(figsize=(max(8, len(vias) * 1.5), max(5, len(pmms) * 0.8)))
    im = ax.imshow(matrix, aspect="auto", cmap="RdYlGn_r")
    ax.set_xticks(range(len(vias)), vias, fontsize=9)
    ax.set_yticks(range(len(pmms)), pmms, fontsize=9)
    for r in range(len(pmms)):
        for c in range(len(vias)):
            v = matrix[r, c]
            if not np.isnan(v):
                ax.text(c, r, f"{v:.1f}", ha="center", va="center", fontsize=8,
                        color="white" if v > np.nanmedian(matrix) else "black")
    fig.colorbar(im, ax=ax, label="mean spread (bps)", shrink=0.8)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Spoof Advantage Heatmap (spread bps)", fontsize=13)
    finish(save_path, block)


# ── 9. Spread Delta (spoofed − direct) ──────────────────────────────────────


def plot_spread_delta(entries, save_path=None, block=False):
    """Difference in spread_bps between each spoof and direct, per PMM."""
    direct_spreads: dict[str, pl.DataFrame] = {}
    for _, via, df in entries:
        if via == "direct":
            df = df.filter(pl.col("spread_bps").is_not_nan())
            if not df.is_empty():
                direct_spreads[df["pmm"][0]] = df

    if not direct_spreads:
        print("  Skipping spread_delta: no direct entries with spread data")
        return

    _, ax = plt.subplots(figsize=(14, 8))
    idx = 0
    for _, via, df in entries:
        if via == "direct":
            continue
        df = df.filter(pl.col("spread_bps").is_not_nan())
        if df.is_empty():
            continue
        pmm = df["pmm"][0]
        if pmm not in direct_spreads:
            continue
        base = direct_spreads[pmm]
        merged = df.join(base.select("amount_in", "spread_bps"), on="amount_in", suffix="_direct")
        if merged.is_empty():
            continue
        delta = (merged["spread_bps"] - merged["spread_bps_direct"]).to_list()
        ax.plot(
            merged["amount_in"], delta,
            linestyle=LINESTYLES[idx % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=f"{pmm} [{via}]", **_marker_kw(idx, len(merged)),
        )
        idx += 1

    ax.axhline(0, color="grey", linewidth=0.8, linestyle="-")
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("spread delta vs direct (bps)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Spread Delta (spoofed − direct)", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 10. CU per Output ───────────────────────────────────────────────────────


def plot_cu_per_output(entries, save_path=None, block=False):
    _, ax = plt.subplots(figsize=(14, 8))
    for i, (_, via, df) in enumerate(entries):
        df = df.filter(pl.col("amount_out") > 0)
        if df.is_empty():
            continue
        cu_per_out = (df["compute_units"].cast(pl.Float64) / df["amount_out"]).to_list()
        ax.plot(
            df["amount_in"], cu_per_out,
            linestyle=LINESTYLES[i % len(LINESTYLES)], linewidth=1.2, alpha=0.85,
            label=make_label(df, via), **_marker_kw(i, len(df)),
        )
    ax.set_xlabel("amount_in", fontsize=11)
    ax.set_ylabel("CU per unit output", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | CU Efficiency (CU / amount_out)", fontsize=13)
    ax.legend(loc="best", **LEGEND_KW)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 11. CU vs Spread Tradeoff (scatter) ─────────────────────────────────────


def plot_cu_vs_spread(entries, save_path=None, block=False):
    """Scatter: each point = one PMM×via, x = mean CU, y = mean spread_bps."""
    points: list[tuple[str, str, float, float]] = []
    for _, via, df in entries:
        df = df.filter(pl.col("spread_bps").is_not_nan())
        if df.is_empty():
            continue
        pmm = df["pmm"][0]
        mean_cu = df["compute_units"].mean()
        mean_spread = df["spread_bps"].mean()
        points.append((pmm, via, mean_cu, mean_spread))

    if not points:
        print("  Skipping cu_vs_spread: no data with spread_bps")
        return

    _, ax = plt.subplots(figsize=(12, 8))
    # Color by PMM, marker by via
    pmm_set = sorted({p[0] for p in points})
    via_set = sorted({p[1] for p in points})
    cmap = plt.colormaps.get_cmap("tab10", len(pmm_set))
    via_markers = {v: MARKERS[i % len(MARKERS)] for i, v in enumerate(via_set)}

    for pmm_name, via, cu, spread in points:
        color = cmap(pmm_set.index(pmm_name))
        ax.scatter(cu, spread, c=[color], marker=via_markers[via], s=80, alpha=0.85,
                   edgecolors="black", linewidths=0.5)

    # Legends
    from matplotlib.lines import Line2D
    pmm_handles = [Line2D([0], [0], marker="o", color="w", markerfacecolor=cmap(i),
                          markersize=8, label=p) for i, p in enumerate(pmm_set)]
    via_handles = [Line2D([0], [0], marker=via_markers[v], color="w", markerfacecolor="grey",
                          markersize=8, label=v) for v in via_set]
    leg1 = ax.legend(handles=pmm_handles, title="PMM", loc="upper left", fontsize=7, title_fontsize=8)
    ax.add_artist(leg1)
    ax.legend(handles=via_handles, title="Via", loc="upper right", fontsize=7, title_fontsize=8)

    ax.set_xlabel("mean compute units", fontsize=11)
    ax.set_ylabel("mean spread (bps)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | CU vs Spread Tradeoff", fontsize=13)
    ax.grid(True, alpha=0.3)
    finish(save_path, block)


# ── 12. PMM Ranking Bar Chart ───────────────────────────────────────────────


def plot_pmm_ranking(entries, save_path=None, block=False):
    """Grouped bar chart: mean spread_bps per PMM, grouped by via-route."""
    data: dict[str, dict[str, float]] = {}
    for _, via, df in entries:
        df = df.filter(pl.col("spread_bps").is_not_nan())
        if df.is_empty():
            continue
        pmm = df["pmm"][0]
        data.setdefault(pmm, {})[via] = df["spread_bps"].mean()

    if not data:
        print("  Skipping pmm_ranking: no spread_bps data")
        return

    pmms = sorted(data.keys())
    vias = sorted({v for d in data.values() for v in d})
    n_vias = len(vias)

    fig, ax = plt.subplots(figsize=(max(10, len(pmms) * 2), 7))
    x = np.arange(len(pmms))
    width = 0.8 / n_vias
    cmap = plt.colormaps.get_cmap("Set2", n_vias)

    for j, via in enumerate(vias):
        vals = [data[p].get(via, 0) for p in pmms]
        ax.bar(x + j * width - 0.4 + width / 2, vals, width, label=via,
               color=cmap(j), edgecolor="black", linewidth=0.4)

    ax.set_xticks(x, pmms, fontsize=9, rotation=30, ha="right")
    ax.set_ylabel("mean spread (bps)", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | PMM Ranking by Spread (bps)", fontsize=13)
    ax.legend(title="via", fontsize=7, title_fontsize=8)
    ax.grid(True, alpha=0.3, axis="y")
    finish(save_path, block)


# ── 13. Max Depth ────────────────────────────────────────────────────────────


def plot_max_depth(entries, save_path=None, block=False, threshold_pct: float = 1.0):
    """Bar chart showing the max amount_in before rate degrades > threshold_pct."""
    depths: list[tuple[str, str, float]] = []
    for _, via, df in entries:
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        base_rate = df["rate"][0]
        if base_rate == 0:
            continue
        pmm = df["pmm"][0]
        # Find last amount_in where rate is within threshold
        impact = ((df["rate"] - base_rate) / base_rate * 100).abs()
        within = df.filter(impact <= threshold_pct)
        max_amt = within["amount_in"].max() if not within.is_empty() else 0
        depths.append((pmm, via, max_amt))

    if not depths:
        print("  Skipping max_depth: no data")
        return

    pmms = sorted({d[0] for d in depths})
    vias = sorted({d[1] for d in depths})
    n_vias = len(vias)

    fig, ax = plt.subplots(figsize=(max(10, len(pmms) * 2), 7))
    x = np.arange(len(pmms))
    width = 0.8 / n_vias
    cmap = plt.colormaps.get_cmap("Set2", n_vias)

    depth_map = {(d[0], d[1]): d[2] for d in depths}
    for j, via in enumerate(vias):
        vals = [depth_map.get((p, via), 0) for p in pmms]
        ax.bar(x + j * width - 0.4 + width / 2, vals, width, label=via,
               color=cmap(j), edgecolor="black", linewidth=0.4)

    ax.set_xticks(x, pmms, fontsize=9, rotation=30, ha="right")
    ax.set_ylabel(f"max amount_in within {threshold_pct}% impact", fontsize=11)
    ax.set_title(f"{_title_prefix(entries[0][2])} | Liquidity Depth ({threshold_pct}% price impact threshold)", fontsize=13)
    ax.legend(title="via", fontsize=7, title_fontsize=8)
    ax.grid(True, alpha=0.3, axis="y")
    finish(save_path, block)


# ── main ─────────────────────────────────────────────────────────────────────

ALL_PLOTS = {
    "exchange_rate": plot_exchange_rate,
    "compute_units": plot_compute_units,
    "spread": plot_spread,
    "spread_bps": plot_spread_bps,
    "price_impact": plot_price_impact,
    "effective_output": plot_effective_output,
    "rate_deviation": plot_rate_deviation,
    "spoof_heatmap": plot_spoof_heatmap,
    "spread_delta": plot_spread_delta,
    "cu_per_output": plot_cu_per_output,
    "cu_vs_spread": plot_cu_vs_spread,
    "pmm_ranking": plot_pmm_ranking,
    "max_depth": plot_max_depth,
}

if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Generate all PMM benchmark visualisations",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=f"Available plot types: {', '.join(ALL_PLOTS.keys())}",
    )
    parser.add_argument("files", nargs="+", help="Parquet files to plot")
    parser.add_argument("--save", default=None, metavar="DIR", help="Save PNGs to this directory")
    parser.add_argument("--no-display", action="store_true", help="Don't show interactive windows (save only)")
    parser.add_argument(
        "--only", default=None,
        help="Comma-separated list of plot types to generate (default: all)",
    )
    parser.add_argument(
        "--depth-threshold", type=float, default=1.0,
        help="Price impact threshold %% for max_depth plot (default: 1.0)",
    )
    args = parser.parse_args()

    if args.no_display:
        import matplotlib
        matplotlib.use("Agg")

    entries = load_files(args.files)
    if not entries:
        print("No files with records to plot")
        exit(0)

    print(f"Loaded {len(entries)} datasets")

    plots_to_run = ALL_PLOTS
    if args.only:
        selected = [s.strip() for s in args.only.split(",")]
        unknown = [s for s in selected if s not in ALL_PLOTS]
        if unknown:
            print(f"Unknown plot types: {unknown}")
            print(f"Available: {list(ALL_PLOTS.keys())}")
            exit(1)
        plots_to_run = {k: v for k, v in ALL_PLOTS.items() if k in selected}

    def _save(name: str) -> str | None:
        return os.path.join(args.save, f"{name}.png") if args.save else None

    n = len(plots_to_run)
    for i, (name, fn) in enumerate(plots_to_run.items()):
        is_last = (i == n - 1) and not args.no_display
        print(f"[{i+1}/{n}] Generating {name}...")
        kwargs = dict(save_path=_save(name), block=is_last)
        if name == "max_depth":
            kwargs["threshold_pct"] = args.depth_threshold
        fn(entries, **kwargs)

    print(f"\nDone! Generated {n} plots.")
    if args.save:
        print(f"Saved to: {args.save}/")
