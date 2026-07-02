# Paseo substitute relay — validator key submission

We're launching a fresh Paseo relay chain from genesis and need your validator's **public keys**
to bake into the genesis. This takes ~2 minutes.

> The chain spec doesn't exist yet — we build it *from* the keys you send. So you generate keys
> now, send us the public parts, and later launch with the final spec we publish. The only thing
> you must carry across is your **keystore** (see "Keep your keys").

## What we need from you

1. **Stash** — your validator account address (SS58).
2. **Session keys** — one blob your node generates (it contains all six keys we need).

## How to get them (pick one)

### Option A — command line

```bash
# 1. Start a node NOW to generate keys. Any current Paseo node build is fine (the spec doesn't
#    exist yet); the important part is --keystore-path, a directory you will KEEP and reuse at
#    launch. This makes your keys independent of the chain spec.
polkadot --chain paseo --validator --name "<your-name>" \
  --keystore-path /srv/paseo-substitute-keys

# 2. In another shell on the SAME machine, generate + store all session keys and print the public blob:
curl -H 'Content-Type: application/json' \
  -d '{"id":1,"jsonrpc":"2.0","method":"author_rotateKeys","params":[]}' \
  http://127.0.0.1:9944
# -> {"jsonrpc":"2.0","result":"0x............","id":1}
#    the 0x value is your SESSION KEYS

# 3. If you don't already have a stash account, create one (keep the secret phrase safe):
polkadot key generate
# -> Secret phrase: ...        <- keep private
#    SS58 Address:  5....       <- this is your STASH

# 4. Switch the node off, send us the stash + session keys, and wait for the final spec.

# 5. LATER — launch with the final spec and the SAME --keystore-path:
polkadot --chain paseo-substitute.json --validator \
  --keystore-path /srv/paseo-substitute-keys
```

### Option B — polkadot.js Apps (browser, no CLI)

1. Start your node with `--keystore-path /srv/paseo-substitute-keys` (as in step 1), open
   https://polkadot.js.org/apps and connect it to **your own** node.
2. **Developer → RPC calls → `author` → `rotateKeys()` → Submit** → copy the returned `0x…` (**session keys**).
3. **Accounts** → copy your validator account address (**stash**).
4. At launch, run the final spec with the same `--keystore-path`.

## What to send back

```
Provider:      <your name / URL>
Stash (SS58):  5..................................................
Session keys:  0x................................................. (the rotateKeys output)
```

## Keep your keys (important)

- `author_rotateKeys` writes your **private** keys into the keystore directory. The `0x` blob you
  send us is **public** and cannot be used to recover them.
- **Use `--keystore-path <dir>` and reuse the exact same directory at launch.** This decouples your
  keys from the chain spec, so it's fine that you generate them before the spec exists. Back that
  directory up; in containers, mount it as a **persistent volume**.
- (If you skip `--keystore-path` and rely on `--base-path`, the keys are stored per-chain-id and
  the substitute node — a different chain-id — won't find them. Use `--keystore-path`.)
- **Send public data only** — never your secret phrase or keystore files.
- Run `rotateKeys` **over `127.0.0.1`** — it's an unsafe RPC; don't expose it publicly.
- If the keystore is lost after we've built genesis, there is **no recovery** — your genesis keys
  would have no matching secret and you'd be an idle validator. Tell us and regenerate **before** launch.
- One submission per validator. Running more than one? Send one stash + session-keys pair per node
  (each with its own `--keystore-path`).
