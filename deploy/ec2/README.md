# EC2 deployment

This bundle installs the `synchronizer` and `relayer` directly on a Debian 13 EC2 instance with:

- local Postgres
- systemd services
- binaries built from this repo
- no Docker

It assumes:

- you already cloned the repo onto the EC2 machine
- you want the deployed checkout under `/srv/zkcraft/repo`
- you are running Debian 13 with `systemd`

## Recommended instance

For one box running `synchronizer`, `relayer`, and local Postgres, start with:

- `t3a.medium`
- `50 GB gp3`

## Bootstrap

Run as root from the repo:

```bash
sudo ./deploy/ec2/bootstrap.sh
```

What it does:

1. Installs system packages, Postgres, Rust toolchain prerequisites, and `rsync`
2. Creates a system user named `zkcraft`
3. Syncs the repo into `/srv/zkcraft/repo`
4. Installs Rust for the `zkcraft` user
5. Builds `synchronizer` and `relayer`
6. Creates local Postgres databases named `synchronizer` and `relayer`
7. Installs env files into `/etc/zkcraft/`
8. Installs systemd units

The script generates a local Postgres password and fills the DB URLs automatically.

## Required edits

After bootstrap, edit:

- `/etc/zkcraft/synchronizer.env`
- `/etc/zkcraft/relayer.env`

Replace the placeholder values for:

- In `/etc/zkcraft/synchronizer.env`:
  - `RPC_URL`
  - `BEACON_URL`
  - `TO_ADDRESS`
- In `/etc/zkcraft/relayer.env`:
  - `RPC_URL`
  - `TO_ADDRESS`
  - `PRIVATE_KEY`

`TO_ADDRESS` must match in both env files.

## Start services

```bash
sudo systemctl restart synchronizer relayer
sudo systemctl status synchronizer relayer
```

Logs:

```bash
journalctl -u synchronizer -f
journalctl -u relayer -f
```

## Service endpoints

- synchronizer: `http://YOUR_HOST:3000`
- relayer: `http://YOUR_HOST:3200`

On AWS, you must open the EC2 security group inbound rules for these ports or the services will only be reachable from the instance itself.

- TCP `3000` to `0.0.0.0/0`
- TCP `3200` to `0.0.0.0/0`

## Updating after a git pull

Re-run the bootstrap script:

```bash
sudo ./deploy/ec2/bootstrap.sh
```

That will resync the repo into `/srv/zkcraft/repo`, rebuild the binaries, and reinstall the systemd units without overwriting existing env files.

## Resetting local state

To recreate the local Postgres databases and clear the synchronizer RocksDB state:

```bash
sudo ./deploy/ec2/reset-db.sh
```

This always resets Postgres, clears `/var/lib/zkcraft/synchronizer-db`, and restarts `synchronizer` and `relayer`.
