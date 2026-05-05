# acuity-ipfs-pinner

A small Rust service that subscribes to live `Content.PublishRevision` events from `acuity-index` and pins the referenced IPFS content to a local Kubo node.

## What it does

- connects to an `acuity-index` WebSocket API
- fetches event metadata with `acuity_getEventMetadata`
- discovers the `Content.PublishRevision` variant index dynamically
- subscribes to live events with `acuity_subscribeEvents`
- reads `decodedEvent.event.fields.ipfs_hash`
- converts the on-chain 32-byte digest hex into a normal CIDv0 string
- calls the local Kubo API to pin the CID
- stores the Kubo repo at `~/.local/share/acuity-ipfs-pinner/ipfs-repo`
- starts `ipfs daemon --repo-dir <repo>` automatically and waits for the API to become available

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
5. For each live notification:
   - verify it is a `Content.PublishRevision`
   - extract `fields.ipfs_hash`
   - convert the digest to CIDv0
   - call Kubo `pin/add`
6. If the WebSocket connection drops, retry after a short delay.

## IPFS hash conversion

The chain event stores `ipfs_hash` as a 32-byte digest hex value.
This service converts it to a CIDv0 by:

- decoding the hex into 32 bytes
- prepending the sha2-256 multihash prefix: `0x12 0x20`
- base58btc-encoding the result

This matches the logic used in `acuity-dioxus`.

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
