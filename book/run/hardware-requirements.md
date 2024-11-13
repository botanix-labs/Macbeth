# Hardware Requirements

## Installation

The Botanix Federation operates on POSIX-based operating systems, including Linux and macOS.
It is recommended for the time being that node operators deploy on non-ARM based Linux machines. ARM based architecture is not supported but will come in the future.

## Hardware requirements

The hardware requirements for running Reth depend on the node configuration and can change over time as the network grows or new features are implemented.

The most important requirement is by far the disk, whereas CPU and RAM requirements are relatively flexible.

|           | Full Node                             |
| --------- | ------------------------------------- |
| Disk      | At least 1.2TB (TLC NVMe recommended) |
| Memory    | 8GB+                                  |
| CPU       | Higher clock speeds over core count   |
| Bandwidth | Stable 24Mbps+                        |

## Disk

There are multiple types of disks to sync Reth, with varying size requirements, depending on the syncing mode.
As of April 2024 at block number 19.6M:

-   Archive Node: At least 2.14TB is required
-   Full Node: At least 1.13TB is required

NVMe drives are recommended for the best performance, with SSDs being a cheaper alternative. HDDs are the cheapest option, but they will take the longest to sync, and are not recommended.

As of February 2024, syncing an Ethereum mainnet node to block 19.3M on NVMe drives takes about 50 hours, while on a GCP "Persistent SSD" it takes around 5 days.

> **Note - QLC and TLC**
>
> It is highly recommended to choose a TLC drive when using NVMe, and not a QLC drive. A list of recommended drives can be found [here](https://gist.github.com/yorickdowne/f3a3e79a573bf35767cd002cc977b038).
> It is crucial to understand the difference between QLC and TLC NVMe drives when considering the disk requirement.
> QLC (Quad-Level Cell) NVMe drives utilize four bits of data per cell, allowing for higher storage density and lower manufacturing costs. However, this increased density comes at the expense of performance. QLC drives have slower read and write speeds compared to TLC drives. They also have a lower endurance, meaning they may have a shorter lifespan and be less suitable for heavy workloads or constant data rewriting.
> TLC (Triple-Level Cell) NVMe drives, on the other hand, use three bits of data per cell. While they have a slightly lower storage density compared to QLC drives, TLC drives offer faster performance. They typically have higher read and write speeds, making them more suitable for demanding tasks such as data-intensive applications, gaming, and multimedia editing. TLC drives also tend to have a higher endurance, making them more durable and longer-lasting.

## CPU

Most of the time spent during syncing is used to execute transactions, a single-threaded operation due to potential state dependencies of one transaction on previous ones. As a result, the number of cores matters less, but in general higher clock speeds are better. More cores are better for parallelizable [stages](https://github.com/paradigmxyz/reth/blob/main/docs/crates/stages.md) (like sender recovery or bodies downloading), but these stages are not the primary bottleneck for syncing.

## Memory

It is recommended to use at least 8GB of RAM.

Most of Reth's components tend to consume a low amount of memory, unless you are under heavy RPC load, so this should matter less than the other requirements.

Higher memory is generally better as it allows for better caching, resulting in less stress on the disk.

## Bandwidth

A stable and dependable internet connection is crucial for both syncing a node from genesis and for keeping up with the chain's tip.

Note that due to Reth's staged sync, you only need an internet connection for the Headers and Bodies stages. This means that the first 1-3 hours (depending on your internet connection) will be online, downloading all necessary data, and the rest will be done offline and does not require an internet connection.

Once you're synced to the tip you will need a reliable connection, especially if you're operating a validator. A 24Mbps connection is recommended, but you can probably get away with less. Make sure your ISP does not cap your bandwidth.