# Blob archiver

A service that archives blobs filtered by destination address.

The node connects to an Ethereum and Beacon RPC nodes to follow beacon blocks and downloads the blobs that were sent via transactions if they match the configured destination address.  At the same time the node offers an http API to serve the stored blobs.

## API endpoints

- `/healthz` -> `{ ok : true }`
- `/header` -> `{ root: B256, parent_root: B256, slot: u32 }`
  - Returns the latest synced beacon block metadata
- `/config` -> `{ filter_address: Address }`
  - Returns the filtering address
- `/v1/beacon/blobs/{block_id}` with query array via `versioned_hashes` -> `{ data: Vec<Blob> }`
  - Follows the same API as the [Ethereum Beacon Node
    API](https://ethereum.github.io/beacon-APIs/#/Beacon/getBlobs) with two
    limitations: only <hex encoded blockRoot with 0x prefix> is supported for
    `block_id` and passing `versioned_hashes` is mandatory.
    This way the archiver can be use as a drop-in replacement of a beacon node for
    getting blobs by their block root and versioned hashes.

## Data structure

All data is stored in the file system (no database required) following this schema:

- `slot_dir`: `{BLOBS_PATH}/by_slot/{slot_hi}/{slot_med}/{slot_lo}` symlink to corresponding `root_dir`
- `root_dir`: `{BLOBS_PATH}/by_root/{root_hi}/{root_med}/{root_lo}`
  - `header.json`: Beacon Block metadata
  - `blob-{index}_{versioned_hash}.bin`: Blob data.  The file name contains the index within the beacon block and its versioned hash.

Example:
```
/tmp/blobs/
├── by_root
│   ├── 0x0f7
│   │   └── 302
│   │       └── 56c45eb94efd0e063d297266b41badb2b484dba7a9ec3617f4a39a2e25
│   │           └── header.json
│   ├── 0x1cc
│   │   └── a7d
│   │       └── 38f52127f8ba2b85e1cc9187bb5f62545fe8b50897dc9a88800eaee42c
│   │           └── header.json
│   ├── 0x523
│   │   └── 9ba
│   │       └── c105b7d49ad4dff66827cb174b506f078e6e9c9890366debbda27ffde8
│   │           └── header.json
│   ├── 0x987
│   │   └── 71c
│   │       └── 3f4ff6b3465734892fa018830f725d4e5b2ae6436e2504b2721e95b72b
│   │           ├── blob-04_0x0184c5bb0836d39ed72caff92e1f88ebba859e1aaf7aca3265ed72737aa86071.bin
│   │           └── header.json
│   ├── 0xdd0
│   │   └── b39
│   │       └── 744411bc7bfb39481a36c5caf3eba527ca4eb869b27ac130a6b4f7d7e1
│   │           └── header.json
│   └── 0xffe
│       └── 67e
│           └── 361dda71df4181205138942f880589d5ed2e9465d6e91e5c4ef0016e03
│               └── header.json.tmp
└── by_slot
    └── 010
        └── 413
            ├── 441 -> ../../../by_root/0x987/71c/3f4ff6b3465734892fa018830f725d4e5b2ae6436e2504b2721e95b72b
            ├── 442 -> ../../../by_root/0x523/9ba/c105b7d49ad4dff66827cb174b506f078e6e9c9890366debbda27ffde8
            ├── 443 -> ../../../by_root/0xdd0/b39/744411bc7bfb39481a36c5caf3eba527ca4eb869b27ac130a6b4f7d7e1
            ├── 444 -> ../../../by_root/0x0f7/302/56c45eb94efd0e063d297266b41badb2b484dba7a9ec3617f4a39a2e25
            ├── 445 -> ../../../by_root/0x1cc/a7d/38f52127f8ba2b85e1cc9187bb5f62545fe8b50897dc9a88800eaee42c
            └── 446 -> ../../../by_root/0xffe/67e/361dda71df4181205138942f880589d5ed2e9465d6e91e5c4ef0016e03
```
