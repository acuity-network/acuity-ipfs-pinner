# CID push/ACK flow

This document explains how the pinner's ACK mechanism works and how a Helia-based publishing node can use it.

## Overview

The pinner supports a custom libp2p protocol:

```text
/acuity/ack/1.0.0
```

A publishing node can:

1. open a stream to the pinner on that protocol
2. send a CID as UTF-8 text
3. wait for an ACK response

The pinner will only send the ACK after it has successfully received the content for that CID.

## Important behavior

This ACK mechanism does **not** pin content.

It only confirms that the pinner was able to fetch the content.

Actual pinning still happens only when:

- the CID is later referenced by a chain `Content.PublishRevision` event
- the pinner sees that event from `acuity-index`
- the pinner then performs its normal pinning flow

So this mechanism is best thought of as:

- **push CID now**
- **confirm the pinner can fetch it now**
- **pin later when the chain event appears**

## Architecture

The pinner service is not itself a native libp2p node.

Instead, it uses the local Kubo daemon as the libp2p endpoint.

### Components

- **Publishing node**: a Helia/libp2p node that dials the ACK protocol
- **Kubo**: receives inbound libp2p streams for the ACK protocol
- **Pinner service**: receives forwarded streams from Kubo, fetches content, and writes the ACK

### Data flow

```text
Publishing node --libp2p stream--> Kubo --local forwarded TCP stream--> pinner service
pinner service --HTTP API cat--> Kubo/IPFS
pinner service --ACK on forwarded stream--> Kubo --libp2p stream--> Publishing node
```

## How the pinner is wired

On startup, the pinner:

1. starts or connects to a local Kubo daemon
2. enables Kubo libp2p stream mounting
3. registers a stream forward using Kubo's `p2p/listen`
4. listens on a local TCP socket for forwarded streams

The configured protocol defaults to:

```text
/acuity/ack/1.0.0
```

Internally, the flow is:

1. remote peer opens a libp2p stream to Kubo using `/acuity/ack/1.0.0`
2. Kubo forwards that stream to the pinner's local TCP listener
3. the pinner reads the stream payload as a CID string
4. the pinner calls Kubo `cat <cid>`
5. if `cat` succeeds, the pinner replies:

```text
ACK: received <cid>
```

6. if `cat` fails, no success ACK is returned

## ACK semantics

The ACK means:

- the pinner received the CID
- the pinner successfully fetched the CID's content through Kubo

The ACK does **not** mean:

- the CID has been pinned
- the CID has appeared on chain
- image mixins have been pinned
- the full chain-event processing pipeline has completed

## Helia publisher example

Below is a minimal Helia example that:

1. creates a Helia node
2. ensures content is available from that node
3. dials the pinner's ACK protocol
4. sends the CID
5. waits for the ACK

## Prerequisites

You need:

- the pinner running
- the pinner's Kubo peer ID
- at least one reachable multiaddr for the pinner's Kubo node
- the publisher node to be providing the CID's content

The pinner logs WebSocket swarm addresses with `/p2p/<peer-id>` on startup. Those addresses can be used by the publisher to connect.

## Example code

```ts
import { createHelia } from 'helia'
import { unixfs } from '@helia/unixfs'
import { pipe } from 'it-pipe'
import all from 'it-all'

const protocol = '/acuity/ack/1.0.0'

async function main() {
  const helia = await createHelia()
  const fs = unixfs(helia)

  // Example: add content locally so this node can provide it.
  const bytes = new TextEncoder().encode('hello from publisher')
  const cid = await fs.addBytes(bytes)

  console.log('published CID:', cid.toString())

  // Replace with an actual multiaddr logged by the pinner's Kubo node.
  // Example only:
  const pinnerAddr = '/ip4/127.0.0.1/tcp/4002/ws/p2p/12D3KooWExamplePeerId'

  await helia.libp2p.peerStore.merge(
    pinnerAddr.split('/p2p/').pop()!,
    {
      multiaddrs: [new URLMultiaddrShim(pinnerAddr)]
    }
  )

  const stream = await helia.libp2p.dialProtocol(pinnerAddr, protocol)

  await pipe(
    [new TextEncoder().encode(cid.toString())],
    stream
  )

  const ackChunks = await all(stream.source)
  const ackBytes = concatChunks(ackChunks)
  const ack = new TextDecoder().decode(ackBytes)

  console.log('ack:', ack)

  if (ack.trim() !== `ACK: received ${cid.toString()}`) {
    throw new Error(`unexpected ACK: ${ack}`)
  }

  await helia.stop()
}

function concatChunks(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0)
  const out = new Uint8Array(total)
  let offset = 0

  for (const chunk of chunks) {
    out.set(chunk, offset)
    offset += chunk.length
  }

  return out
}

// Tiny placeholder so the example reads clearly.
// In real code, use `multiaddr()` from `@multiformats/multiaddr`.
function URLMultiaddrShim(addr: string): any {
  return addr
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
```

## Recommended Helia example using `multiaddr`

This is the same example using the normal multiaddr helper:

```ts
import { createHelia } from 'helia'
import { unixfs } from '@helia/unixfs'
import { multiaddr } from '@multiformats/multiaddr'
import all from 'it-all'
import { pipe } from 'it-pipe'

const protocol = '/acuity/ack/1.0.0'

async function main() {
  const helia = await createHelia()
  const fs = unixfs(helia)

  const cid = await fs.addBytes(new TextEncoder().encode('hello from publisher'))
  console.log('cid:', cid.toString())

  const pinnerAddr = multiaddr(
    '/ip4/127.0.0.1/tcp/4002/ws/p2p/12D3KooWExamplePeerId'
  )

  const stream = await helia.libp2p.dialProtocol(pinnerAddr, protocol)

  await pipe(
    [new TextEncoder().encode(cid.toString())],
    stream.sink
  )

  const chunks = await all(stream.source)
  const ack = new TextDecoder().decode(concatChunks(chunks)).trim()

  console.log('ack:', ack)

  if (ack !== `ACK: received ${cid.toString()}`) {
    throw new Error(`unexpected ACK: ${ack}`)
  }

  await helia.stop()
}

function concatChunks(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0)
  const out = new Uint8Array(total)
  let offset = 0

  for (const chunk of chunks) {
    out.set(chunk, offset)
    offset += chunk.length
  }

  return out
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
```

## Notes about the Helia side

For the ACK to succeed, the pinner must be able to fetch the CID.

That usually means:

- the publishing Helia node is online
- the publishing Helia node is actually serving the blocks for the CID
- the pinner's Kubo node can connect to the publisher over libp2p

If the publisher sends the CID and immediately disappears, the pinner may fail to fetch it and therefore not send a success ACK.

## Sequence diagram

```text
Helia publisher                    Pinner Kubo                    Pinner service
     |                                 |                               |
     | dialProtocol(protocol)          |                               |
     |-------------------------------->|                               |
     |                                 | forward stream locally        |
     |                                 |------------------------------>|
     | send CID                        |                               |
     |-------------------------------->|------------------------------>|
     |                                 |                               |
     |                                 |        cat(cid)               |
     |                                 |<------------------------------|
     |                                 | serves/fetches through IPFS   |
     |<==================== content retrieval path ===================>|
     |                                 |                               |
     |                                 | ACK: received <cid>           |
     |                                 |<------------------------------|
     | receive ACK                     |                               |
     |<--------------------------------|                               |
```

## What happens later

Later, when the chain emits `Content.PublishRevision`:

1. the pinner sees the event from `acuity-index`
2. converts the on-chain digest to a CID
3. pins that CID through Kubo
4. reads the item bytes back
5. extracts image mixin CIDs
6. pins those image CIDs too

That is the only path that creates pins.

## Configuration

The pinner exposes the ACK protocol as a CLI option:

```bash
cargo run -- \
  --indexer-url ws://127.0.0.1:8172 \
  --kubo-api-url http://127.0.0.1:5001 \
  --ack-protocol /acuity/ack/1.0.0
```

## Summary

The ACK protocol is a lightweight pre-confirmation mechanism:

- publisher pushes a CID
- pinner fetches the CID
- pinner replies with ACK only after fetch succeeds
- no pin is created here
- pinning still depends entirely on the chain event flow
