#!/usr/bin/env python3
"""Plot the exchange rate and compute units for Solana Prop AMM markets."""

import os
import random

import matplotlib.pyplot as plt
import polars as pl

MARKERS = ["o", "s", "^", "v", "*", "h", "<", ">", "p"]
LINESTYLES = ["--", "-.", ":"]
VIA_TAGS = {"magnus", "jupiter", "okxlabs", "dflow", "titan", "direct"}


def _extract_via(filepath: str) -> str | None:
    """Extract the via tag (magnus/jupiter/direct/...) from the filename.

    Filename format: slot_via_pmm_market_time.parquet
    """
    basename = os.path.basename(filepath).removesuffix(".parquet")
    parts = basename.split("_", 2)  # [slot, via, rest...]
    if len(parts) >= 2 and parts[1] in VIA_TAGS:
        return parts[1]
    return None


def _make_label(df: pl.DataFrame, via: str | None) -> str:
    """Short label: 'pmm [via]' — no market address."""
    pmm = df["pmm"][0]
    return f"{pmm} [{via}]" if via else pmm


def _finish_plot(save_path: str | None, block: bool):
    plt.tight_layout()
    if save_path:
        os.makedirs(os.path.dirname(save_path), exist_ok=True)
        plt.savefig(save_path, dpi=150, bbox_inches="tight")
        print(f"Saved: {save_path}")
    plt.show(block=block)


def plot_exchange_rate(
    files: list[str],
    block: bool = True,
    markers: bool = False,
    linestyle: str | None = None,
    save_path: str | None = None,
    ylim: float | None = None,
):
    _, ax = plt.subplots(figsize=(14, 8))

    for i, file in enumerate(files):
        df = pl.read_parquet(file)
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        via = _extract_via(file)
        label = _make_label(df, via)
        kwargs = {}
        if markers:
            kwargs["marker"] = MARKERS[i % len(MARKERS)]
            kwargs["markersize"] = 5
            kwargs["markevery"] = (i * 7, max(10, len(df) // 30))
            kwargs["fillstyle"] = "none"
            kwargs["markeredgewidth"] = 1.2
        ls = linestyle if linestyle else LINESTYLES[i % len(LINESTYLES)]
        ax.plot(
            df["amount_in"],
            df["rate"],
            linestyle=ls,
            linewidth=1.4,
            alpha=0.85,
            label=label,
            **kwargs,
        )

    first_df = pl.read_parquet(files[0])
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | EXCHANGE RATE"

    ax.set_xlabel(f"amount_in ({first_df['src_token'][0]})", fontsize=12)
    ax.set_ylabel("exchange_rate", fontsize=12)
    ax.set_title(title, fontsize=14)
    if ylim is not None:
        ax.set_ylim(top=ylim)
    ax.legend(loc="best", fontsize=8, ncol=2, handlelength=1.5, handletextpad=0.4, columnspacing=1.0)
    ax.grid(True, alpha=0.3)

    _finish_plot(save_path, block)


def plot_compute_units(
    files: list[str],
    block: bool = True,
    markers: bool = False,
    linestyle: str | None = None,
    save_path: str | None = None,
    ylim: float | None = None,
):
    _, ax = plt.subplots(figsize=(14, 8))

    for i, file in enumerate(files):
        df = pl.read_parquet(file)
        via = _extract_via(file)
        label = _make_label(df, via)
        kwargs = {}
        if markers:
            kwargs["marker"] = MARKERS[i % len(MARKERS)]
            kwargs["markersize"] = 5
            kwargs["markevery"] = (i * 7, max(10, len(df) // 30))
            kwargs["fillstyle"] = "none"
            kwargs["markeredgewidth"] = 1.2
        ls = linestyle if linestyle else LINESTYLES[i % len(LINESTYLES)]
        ax.plot(
            df["amount_in"],
            df["compute_units"],
            linestyle=ls,
            linewidth=1.4,
            alpha=0.85,
            label=label,
            **kwargs,
        )

    first_df = pl.read_parquet(files[0])
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | COMPUTE UNITS"

    ax.set_xlabel("amount_in", fontsize=12)
    ax.set_ylabel("compute_units", fontsize=12)
    ax.set_title(title, fontsize=14)
    if ylim is not None:
        ax.set_ylim(top=ylim)
    ax.legend(loc="best", fontsize=8, ncol=2, handlelength=1.5, handletextpad=0.4, columnspacing=1.0)
    ax.grid(True, alpha=0.3)

    _finish_plot(save_path, block)


def plot_spread(
    files: list[str],
    block: bool = True,
    markers: bool = False,
    linestyle: str | None = None,
    bps: bool = False,
    save_path: str | None = None,
    ylim: float | None = None,
):
    _, ax = plt.subplots(figsize=(14, 8))
    col = "spread_bps" if bps else "spread"

    for i, file in enumerate(files):
        df = pl.read_parquet(file)
        df = df.filter(pl.col(col).is_not_nan())
        if df.is_empty():
            print(f"WARNING: skipping '{file}' (no {col} data)")
            continue
        via = _extract_via(file)
        label = _make_label(df, via)
        kwargs = {}
        if markers:
            kwargs["marker"] = MARKERS[i % len(MARKERS)]
            kwargs["markersize"] = 5
            kwargs["markevery"] = (i * 7, max(10, len(df) // 30))
            kwargs["fillstyle"] = "none"
            kwargs["markeredgewidth"] = 1.2
        ls = linestyle if linestyle else LINESTYLES[i % len(LINESTYLES)]
        ax.plot(
            df["amount_in"],
            df[col],
            linestyle=ls,
            linewidth=1.4,
            alpha=0.85,
            label=label,
            **kwargs,
        )

    first_df = pl.read_parquet(files[0])
    suffix = "SPREAD (bps)" if bps else "SPREAD"
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | {suffix}"

    ax.set_xlabel(f"amount_in ({first_df['src_token'][0]})", fontsize=12)
    ax.set_ylabel("spread (bps)" if bps else "spread", fontsize=12)
    ax.set_title(title, fontsize=14)
    if ylim is not None:
        ax.set_ylim(top=ylim)
    ax.legend(loc="best", fontsize=8, ncol=2, handlelength=1.5, handletextpad=0.4, columnspacing=1.0)
    ax.grid(True, alpha=0.3)

    _finish_plot(save_path, block)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Visualise PMM benchmark data")
    parser.add_argument("files", nargs="+", help="Parquet files to plot")
    parser.add_argument(
        "--type", choices=["rate", "compute", "spread", "spread_bps", "all"], default="all", help="Plot type"
    )
    parser.add_argument(
        "--markers", action="store_true", help="Show markers on data points"
    )
    parser.add_argument(
        "--linestyle",
        choices=["-", "--", "-.", ":"],
        default=None,
        help="Line style (default: cycles per dataset)",
    )
    parser.add_argument(
        "--save",
        default=None,
        metavar="DIR",
        help="Save plots as PNGs to this directory instead of only displaying",
    )
    parser.add_argument(
        "--ylim", type=float, default=None,
        help="Cap the y-axis at this value",
    )
    parser.add_argument(
        "--exclude", default=None,
        help="Comma-separated PMM names to exclude (e.g. solfi-v2,goonfi)",
    )
    parser.add_argument(
        "--exclude-via", default=None,
        help="Comma-separated via-routes to exclude (e.g. dflow,titan)",
    )
    args = parser.parse_args()

    exclude_pmms = set()
    if args.exclude:
        exclude_pmms = {s.strip().lower() for s in args.exclude.split(",")}

    exclude_vias = set()
    if args.exclude_via:
        exclude_vias = {s.strip().lower() for s in args.exclude_via.split(",")}

    valid_files = []
    for f in args.files:
        if not os.path.exists(f):
            print(f"File not found: {f}")
            exit(1)
        # Check via exclusion from filename
        via = _extract_via(f)
        if via and via.lower() in exclude_vias:
            continue
        df = pl.read_parquet(f)
        if df.is_empty():
            print(f"WARNING: skipping '{f}' (no records)")
            continue
        # Check PMM exclusion from data
        pmm = df["pmm"][0].lower()
        if pmm in exclude_pmms:
            continue
        valid_files.append(f)

    if not valid_files:
        print("No files with records to plot")
        exit(0)

    save_dir = args.save
    plot_kwargs = dict(markers=args.markers, linestyle=args.linestyle, ylim=args.ylim)

    def _save(name: str) -> str | None:
        return os.path.join(save_dir, f"{name}.png") if save_dir else None

    if args.type == "all":
        plot_compute_units(valid_files, block=False, save_path=_save("compute_units"), **plot_kwargs)
        plot_spread(valid_files, block=False, save_path=_save("spread"), **plot_kwargs)
        plot_spread(valid_files, block=False, bps=True, save_path=_save("spread_bps"), **plot_kwargs)
        plot_exchange_rate(valid_files, block=True, save_path=_save("exchange_rate"), **plot_kwargs)
    elif args.type == "rate":
        plot_exchange_rate(valid_files, save_path=_save("exchange_rate"), **plot_kwargs)
    elif args.type == "compute":
        plot_compute_units(valid_files, save_path=_save("compute_units"), **plot_kwargs)
    elif args.type == "spread":
        plot_spread(valid_files, save_path=_save("spread"), **plot_kwargs)
    elif args.type == "spread_bps":
        plot_spread(valid_files, bps=True, save_path=_save("spread_bps"), **plot_kwargs)
