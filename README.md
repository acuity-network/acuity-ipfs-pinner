# acuity-ipfs-pinner

A small Rust service that subscribes to live `Content.PublishRevision` events from `acuity-index`, pins the referenced IPFS content to a local Kubo node, then inspects the protobuf item payload for image mixins and pins every embedded image CID too.

## What it does

- connects to an `acuity-index` WebSocket API
- fetches event metadata with `acuity_getEventMetadata`
- discovers the `Content.PublishRevision` variant index dynamically
- subscribes to live events with `acuity_subscribeEvents`
- reads `event.event.fields.ipfs_hash`
- converts the on-chain 32-byte digest hex into a normal CIDv0 string
- calls the local Kubo API to pin the revision CID
- fetches the pinned revision bytes back from Kubo with `cat`
- decodes the protobuf `ItemMessage`
- scans `IMAGE_MIXIN_ID = 0x045e_ee8c`
- extracts `ImageMixinMessage.ipfs_hash` and every `mipmap_level[*].ipfs_hash`
- pins every extracted image CID and logs the list
- stores the Kubo repo at `~/.local/share/acuity-ipfs-pinner/ipfs-repo`
- runs `ipfs init --repo-dir <repo>` on startup and logs whether the repo was created or already existed
- runs `ipfs config --json Addresses.Swarm ... --repo-dir <repo>` before starting the daemon
- starts `ipfs daemon --repo-dir <repo>` automatically and waits for the API to become available
- logs each IPv4/IPv6 WebSocket swarm multiaddr reported by Kubo after startup, including the `/p2p/<peer-id>` suffix

Historical events are not handled.

## Assumptions

This program assumes the target `acuity-index` instance is running with:

- a chain spec that includes the `Content` pallet
- `index_variant = true`

Without variant indexing enabled, `acuity-index` cannot subscribe to all `Content.PublishRevision` events as a class.

## Defaults

- acuity-index URL: `ws://127.0.0.1:8172`
- Kubo API URL: `http://127.0.0.1:5001`

## Usage

Run with defaults:

```bash
cargo run --
```

Run with explicit URLs:

```bash
cargo run -- \
  --indexer-url ws://127.0.0.1:8172 \
  --kubo-api-url http://127.0.0.1:5001
```

Build a release binary:

```bash
cargo build --release
./target/release/acuity-ipfs-pinner
```

## Command line options

- `--indexer-url <URL>`: `acuity-index` WebSocket endpoint
- `--kubo-api-url <URL>`: Kubo HTTP API base URL
- `-h`, `--help`: show help

## How it works

1. Connect to `acuity-index` over WebSocket.
2. Request event metadata.
3. Find the pallet/event indexes for `Content.PublishRevision`.
4. Subscribe using the `Variant` key.
5. For each live chain notification:
   - verify it is a hydrated `Content.PublishRevision`
   - extract `event.event.fields.ipfs_hash`
   - convert the digest to CIDv0
   - call Kubo `pin/add`
   - fetch the revision bytes from Kubo
   - decode the protobuf item payload
   - extract and log image CIDs from any image mixins
   - call Kubo `pin/add` for each extracted image CID
6. If the WebSocket connection drops, retry after a short delay.

## IPFS hash conversion

The chain event stores `ipfs_hash` as a 32-byte digest hex value.
This service converts it to a CIDv0 by:

- decoding the hex into 32 bytes
- prepending the sha2-256 multihash prefix: `0x12 0x20`
- base58btc-encoding the result

This matches the logic used in `acuity-dioxus`.

## Image mixin protobuf layout

From the updated content encoding, `ItemMessage` now stores:

- `content_type_id` at field `1`
- repeated `mixin_payload` entries at field `2`

Images are stored inside an `ImageMixinMessage` with:

- `ipfs_hash` at field `3`
- repeated `mipmap_level` entries at field `6`
- each `MipmapLevelMessage` stores its CID multihash bytes in `ipfs_hash` at field `2`

Publishing code now sets `content_type_id` explicitly, but this pinner only needs the image mixins and ignores the specific content type value.

JPEG mipmaps are stored directly in `mipmap_level[*].ipfs_hash`. The top-level `ipfs_hash` may still be empty, and this pinner supports both locations.

## Requirements

- Rust toolchain
- a running `acuity-index` instance
- the `ipfs` CLI installed locally

By default the embedded Kubo daemon uses this repo path:

```text
~/.local/share/acuity-ipfs-pinner/ipfs-repo
```

Example Kubo endpoint:

```text
http://127.0.0.1:5001
```

## Development

Format and test:

```bash
cargo fmt
cargo test
```

Only unit tests are included. Integration tests are intentionally not included.

## Notes

- The service keeps no persistent state.
- Repeated events may cause repeated pin requests; Kubo pinning is expected to be safe for that case.
- Notifications are processed from live subscriptions only.
