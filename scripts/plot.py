#!/usr/bin/env python3
"""Plot the exchange rate and compute units for Solana Prop AMM markets."""

import os

import matplotlib.pyplot as plt
import polars as pl


def plot_exchange_rate(files: list[str], block: bool = True):
    _, ax = plt.subplots(figsize=(12, 8))

    for file in files:
        df = pl.read_parquet(file)
        df = df.with_columns((pl.col("amount_out") / pl.col("amount_in")).alias("rate"))
        label = f"{df['pmm'][0]} ({df['market'][0]})"
        ax.plot(df["amount_in"], df["rate"], marker="o", markersize=3, label=label)

    first_df = pl.read_parquet(files[0])
    title = f"Solana Prop AMM Markets ({first_df['src_token'][0]} → {first_df['dst_token'][0]}) - slot {first_df['slot'][0]} | EXCHANGE RATE"

    ax.set_xlabel(f"amount_in ({first_df['src_token'][0]})", fontsize=12)
    ax.set_ylabel("exchange_rate", fontsize=12)
    ax.set_title(title, fontsize=14)
    ax.legend(loc="best")
    ax.grid(True, alpha=0.3)

    plt.tight_layout()
    plt.show(block=block)


def plot_compute_units(files: list[str], block: bool = True):
    _, ax = plt.subplots(figsize=(12, 8))

    for file in files:
        df = pl.read_parquet(file)
        label = f"{df['pmm'][0]} ({df['market'][0]})"
        ax.plot(
            df["amount_in"], df["compute_units"], marker="o", markersize=3, label=label
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
    args = parser.parse_args()

    for f in args.files:
        if not os.path.exists(f):
            print(f"File not found: {f}")
            exit(1)

    if args.type == "all":
        plot_compute_units(args.files, block=False)
        plot_exchange_rate(args.files, block=True)
    else:
        if args.type == "rate":
            plot_exchange_rate(args.files)
        elif args.type == "compute":
            plot_compute_units(args.files)
