# kernel

The kernel of the Android VM. DSA builds and ships it, so users never build or replace a
kernel, and the user's WSL kernel is not touched.

| File | What it is |
|---|---|
| `wsl-kernel.tag` | The WSL kernel tag the source comes from |
| `android.config` | The options merged on top of that tag's `Microsoft/config-wsl` |
| `patches/` | Small patches applied to the pinned source before configuration |
| `build.sh` | Fetches the source into `.scratch/`, merges the configuration, checks it and builds `bzImage` |

## Why the WSL kernel

The WSL kernel is already tuned for this kind of VM: it carries the Hyper-V drivers
(VMBus, storage, networking, sockets, the balloon driver with free page reporting) and
the configuration Microsoft maintains for a lightweight Linux VM on Windows. Android's
needs come on top as one fragment, so following a new WSL tag is a one-line change.

The source is configured from Microsoft's baseline. A small checked-in patch extends the
command-line partition parser so Hyper-V can provide Android the partition UUID and name
metadata its virtual SCSI uevents omit.

## What the fragment adds

- **Binder and binderfs, built in.** Built in, binder uses the kernel's internal symbols
  directly and creates `/dev/binder`, `/dev/hwbinder` and `/dev/vndbinder` for Android's
  early services. An out-of-tree module on the stock WSL kernel cannot: the driver needs
  about a dozen kernel symbols that the WSL kernel does not export to modules.
- **What Android's userspace expects:** wake locks, dma-buf heaps for graphics buffers,
  utilisation clamps, uinput and evdev, file-based encryption and fs-verity, erofs and f2fs,
  dm-crypt, quotas, and the eBPF and netfilter pieces netd uses.
- **Everything built in.** The VM boots one image with no modules, so options
  `config-wsl` builds as modules, and their dependencies, are set to `y`.
- **Bootconfig**, which carries each boot's control channel secret from the host. The
  guest reads it from `/proc/bootconfig`, which apps cannot read.
- **Its own release name** (`-dsa`), so it is never mistaken for the WSL kernel.
- **pstore on a disk (pstore/blk)**, so the kernel's last messages, an oops's report and
  Android's last log lines (pmsg) are kept on a small disk and shown in `/sys/fs/pstore` at
  the next boot. `dsa logs collect` carries what is there, and the guest then clears it.

## The pstore disk

A panic ends the VM: HCS runs it with `StopOnReset`, and `panic=-1` restarts the guest at
once, so the next start is a new VM with empty memory. A log kept in RAM (ramoops) would be
gone by then, so the kernel keeps it on a disk instead.

The host attaches `pstore.vhd`, beside the user's Android disk, as the last disk of the VM
(`dsa/host/platform/windows/src/pstore_disk.rs`). It is a fixed VHD of 4 MiB, so it never
grows, with one GPT partition whose UUID is fixed. The host names that partition on the kernel
command line:

```
pstore_blk.blkdev=PARTUUID=5f8b3c2e-9a41-4d6e-b07c-1e2d3f4a5b6c pstore_blk.best_effort=y
pstore_blk.kmsg_size=64 pstore_blk.console_size=256 pstore_blk.pmsg_size=256
```

- **Found by UUID:** the guest names its disks in the order they finish probing, so the
  partition UUID finds the disk whatever its name.
- **Waiting for the disk:** pstore/blk looks its disk up at `late_initcall`, and Hyper-V's
  storage and SCSI disk drivers probe asynchronously, so the disk may not exist yet.
  `patches/pstore-blk-wait-for-disk.patch` waits for the probes started so far first, as
  mounting the root file system does.
- **How it is split:** the console and pmsg take 256 KiB each, and the rest of the partition
  (about 2.5 MiB) holds oops records of 64 KiB, the oldest replaced first.
- **What it costs:** no guest memory is reserved, unlike ramoops' region.

What a panic leaves, and what it does not: pstore/blk writes through the normal block path,
which cannot run once the kernel has panicked (Hyper-V's storage driver has no panic write).
The console and pmsg zones are written as they fill, and from atomic context about half a
second later, so they hold the log up to just before the panic. An oops the kernel survives is
written whole. The panic's own report is not; it reaches the host on the serial console, which
the host keeps in the console log and `dsa logs collect` carries (`host/android-console.log`).

Once the host has the records, the guest unlinks them from `/sys/fs/pstore`, which erases them
from the disk, so the next collection does not carry them again.
ashmem is not enabled and does not exist in this kernel; Android uses memfd.

Two options of Android's own kernel are not in the upstream source: `DM_DEFAULT_KEY`
(metadata encryption) and `INCREMENTAL_FS` (streaming app installs). The guest image is
configured not to need them.

## Building

```
dsa/kernel/build.sh --config-only   # merge and check, about a minute
dsa/kernel/build.sh                 # also build bzImage, tens of minutes
```

The check fails when any line of `android.config` did not survive the merge, which is how
Kconfig reports a missing dependency: by silently dropping the option. It also fails if
ashmem is enabled or memfd is not.

CI runs the full build in "Kernel test" (`.github/workflows/test-kernel.yml`) when these
files change, by hand, and weekly. The weekly run warns when a newer WSL kernel tag
exists. To try one, run the workflow with its `tag` input, then update `wsl-kernel.tag`.
