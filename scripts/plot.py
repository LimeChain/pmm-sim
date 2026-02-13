#!/usr/bin/env python3
"""Plot the exchange rate and compute units for Solana Prop AMM markets."""

import os
import random

import matplotlib.pyplot as plt
import polars as pl

MARKERS = ["o", "s", "^", "D", "v", "P", "X", "*", "h", "<", ">", "p"]
LINESTYLES = ["--", "-.", ":"]


def plot_exchange_rate(
    files: list[str],
    block: bool = True,
    markers: bool = False,
    linestyle: str | None = None,
):
    _, ax = plt.subplots(figsize=(12, 8))

    for i, file in enumerate(files):
        df = pl.read_parquet(file)
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        label = f"{df['pmm'][0]} ({df['market'][0]})"
        kwargs = {}
        if markers:
            kwargs["marker"] = MARKERS[i % len(MARKERS)]
            kwargs["markersize"] = 6
            kwargs["markevery"] = random.randint(5, 30)
        ls = linestyle if linestyle else "-"
        ax.plot(
            df["amount_in"],
            df["rate"],
            linestyle=ls,
            linewidth=1,
            label=label,
            **kwargs,
        )

    first_df = pl.read_parquet(files[0])
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | EXCHANGE RATE"

    ax.set_xlabel(f"amount_in ({first_df['src_token'][0]})", fontsize=12)
    ax.set_ylabel("exchange_rate", fontsize=12)
    ax.set_title(title, fontsize=14)
    ax.legend(loc="best")
    ax.grid(True, alpha=0.3)

    plt.tight_layout()
    plt.show(block=block)


def plot_compute_units(
    files: list[str],
    block: bool = True,
    markers: bool = False,
    linestyle: str | None = None,
):
    _, ax = plt.subplots(figsize=(12, 8))

    for i, file in enumerate(files):
        df = pl.read_parquet(file)
        label = f"{df['pmm'][0]} ({df['market'][0]})"
        kwargs = {}
        if markers:
            kwargs["marker"] = MARKERS[i % len(MARKERS)]
            kwargs["markersize"] = 6
            kwargs["markevery"] = random.randint(5, 30)
        ls = linestyle if linestyle else "-"
        ax.plot(
            df["amount_in"],
            df["compute_units"],
            linestyle=ls,
            linewidth=1,
            label=label,
            **kwargs,
        )

    first_df = pl.read_parquet(files[0])
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | COMPUTE UNITS"

    ax.set_xlabel("amount_in", fontsize=12)
    ax.set_ylabel("compute_units", fontsize=12)
    ax.set_title(title, fontsize=14)
    ax.legend(loc="best")
    ax.grid(True, alpha=0.3)

    plt.tight_layout()
    plt.show(block=block)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Visualise PMM benchmark data")
    parser.add_argument("files", nargs="+", help="Parquet files to plot")
    parser.add_argument(
        "--type", choices=["rate", "compute", "all"], default="all", help="Plot type"
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
    args = parser.parse_args()

    valid_files = []
    for f in args.files:
        if not os.path.exists(f):
            print(f"File not found: {f}")
            exit(1)
        df = pl.read_parquet(f)
        if df.is_empty():
            print(f"WARNING: skipping '{f}' (no records)")
        else:
            valid_files.append(f)

    if not valid_files:
        print("No files with records to plot")
        exit(0)

    plot_kwargs = dict(markers=args.markers, linestyle=args.linestyle)

    if args.type == "all":
        plot_compute_units(valid_files, block=False, **plot_kwargs)
        plot_exchange_rate(valid_files, block=True, **plot_kwargs)
    else:
        if args.type == "rate":
            plot_exchange_rate(valid_files, **plot_kwargs)
        elif args.type == "compute":
            plot_compute_units(valid_files, **plot_kwargs)
