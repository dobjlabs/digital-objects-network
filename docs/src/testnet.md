# Testnet

The network currently runs as a testnet anchored to the Ethereum Sepolia
testnet. Objects, proofs, and state on it are for testing only and may be
reset.

## Network parameters

| Parameter    | Value                                                                                                |
| ------------ | ---------------------------------------------------------------------------------------------------- |
| Network      | Sepolia (Ethereum testnet)                                                                           |
| Start slot   | `11491968` — the beacon slot the network began ingesting blobs from; changes with each testnet reset |
| Blob address | `0x4343434343434343434343434343434343434343` — the address blob transactions are posted to           |

Services that follow the testnet need both values; see
[Running the services](./deployment.md).

## Hosted services

These services are run for the testnet. Release builds of the driver use the
hosted synchronizer and relayer by default, so a fresh install needs no
configuration.

| Service      | Endpoint                                                                         | Role                                                                                              |
| ------------ | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Synchronizer | [`synchronizer.digiobjects.net`](https://synchronizer.digiobjects.net/healthz)   | Rebuilds canonical network state from Ethereum blobs; answers live / consumed queries             |
| Relayer      | [`relayer.digiobjects.net`](https://relayer.digiobjects.net/healthz)             | Submits proof payloads to Ethereum as EIP-4844 blob transactions                                  |
| Archiver     | [`terminus.digiobjects.net:4001`](https://terminus.digiobjects.net:4001/healthz) | Stores historical blobs durably so synchronizers can bootstrap beyond Ethereum's retention window |

## Choosing services

A driver can use any synchronizer and relayer, including your own. Set the
endpoints with the `dobj` CLI; the change applies immediately, without a
restart:

```bash
dobj settings set \
  --synchronizer https://synchronizer.digiobjects.net \
  --relayer https://relayer.digiobjects.net
```

`dobj settings get` prints the current values. The driver only talks to a
synchronizer and a relayer; the archiver is consumed by synchronizers (to fetch
historical blobs), so drivers do not configure it.
