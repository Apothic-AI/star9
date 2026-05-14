# Disk, Dossrv, Vacfs Provider Design

Star 9 keeps `dossrv` and `vacfs` as provider-backed commands. They must not be fake command shims that print success without creating real file surfaces.

## Rule

Provider-heavy commands create or mount filesystems through Star 9 namespaces, `#srv`, device files, and 9P-style composition. They do not expose privileged side APIs.

## `dossrv`

Status: provider missing.

Required provider shape:

- A block/disk provider exposes disks, partitions, offsets, sector size, read/write capability, and error state as files or device resources.
- A DOS/FAT filesystem adapter consumes one provider resource and exposes a normal Star 9 filesystem.
- `dossrv` registers the resulting filesystem service in `#srv`.
- `mount service n/name` mounts that real filesystem into the namespace.

Expected command flow:

```text
dossrv [-r] disk-resource service-name
mount service-name n/dos
```

Unsupported until implemented:

- Raw host block device access.
- Partition discovery.
- FAT mutation semantics.
- Browser disk images unless backed by an explicit mounted file/blob/provider.

## `vacfs`

Status: provider missing.

Required provider shape:

- A vac/Venti/archive provider exposes archive roots, blocks, manifests, metadata, and errors as file-backed resources.
- A read-only or writable filesystem adapter exposes archive contents as normal Star 9 files.
- `vacfs` registers or mounts that filesystem through `#srv`/`mount`.

Expected command flow:

```text
vacfs archive-resource service-name
mount service-name n/vac
```

Stepping stones:

- Existing `TarFs` can inform archive-provider plumbing, but `vacfs` should not be reduced to tar semantics.
- Future content-addressed store work should expose hashes, block cache state, and verification errors through small files/control surfaces.

## Current Correct Behavior

Until these providers exist, Star 9 returns precise provider-missing command errors for `dossrv` and `vacfs`. That is intentional and tested. The commands should only move to implemented status after they create or mount real filesystem services.
