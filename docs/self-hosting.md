# Self-hosting your novel's git remote

Quill syncs through a git remote. The desktop pushes to it, and the iPhone app
clones from it and syncs with it (`docs/ios.md`). A hosted service such as
GitHub works, but you don't need one. This guide puts the remote on a machine
you own, with nothing exposed to the internet:

- **[Forgejo](https://forgejo.org/)** is the git server. It's a lightweight,
  community-run forge, a nonprofit fork of Gitea.
- **[Tailscale](https://tailscale.com/)** is a private network between your
  own devices. `tailscale serve` gives Forgejo an HTTPS address with a real
  certificate that only your devices can reach, at home or away.

```
 desktop Quill ─┐                                   ┌─ iPhone Quill
                ├── Tailscale (private, encrypted) ──┤
                └──────────▶  Forgejo on your Mac  ◀─┘
```

Either device can be offline. Edits are committed locally and go out on the
next sync that reaches the server.

## Why Tailscale and not just the home network

On Apple platforms, the app's git library checks HTTPS certificates against
the iPhone's trust store. A self-signed certificate on a local-network server
fails unless you install it on the phone as a trusted profile. A plain local
server also stops working the moment you leave home. Tailscale fixes both. The
phone reaches the server over cellular or any Wi-Fi, through a certificate it
already trusts, and the server stays invisible to everyone else.

Use `tailscale serve`, never `tailscale funnel`. Funnel publishes a service on
the public internet.

## 1. Choose the host

Forgejo must be running whenever a device syncs. A Mac you use every day is
fine to start with. Under **System Settings → Energy**, turn on **Prevent
automatic sleeping when the display is off** so it stays reachable. An
always-on machine (a Mac mini, a NAS, a small home server) is better long term.
The steps are the same.

## 2. Install and configure Forgejo

```sh
brew install forgejo
```

The Homebrew service runs Forgejo with its work path at
`/opt/homebrew/var/forgejo`. Write its configuration before the first start,
so the web installer never runs and nothing listens beyond this machine:

```sh
W=/opt/homebrew/var/forgejo
mkdir -p "$W/custom/conf" "$W/data" "$W/log"
F=/opt/homebrew/opt/forgejo/bin/forgejo
cat > "$W/custom/conf/app.ini" <<EOF
APP_NAME = Quill Forgejo
RUN_MODE = prod
WORK_PATH = $W

[server]
PROTOCOL = http
; Only this machine can reach Forgejo directly; Tailscale proxies to it.
HTTP_ADDR = 127.0.0.1
HTTP_PORT = 3000
; Set to the Tailscale address in step 3.
ROOT_URL = http://localhost:3000/
DISABLE_SSH = true
LFS_START_SERVER = true
LFS_JWT_SECRET = $($F generate secret JWT_SECRET)
OFFLINE_MODE = true

[database]
DB_TYPE = sqlite3
PATH = $W/data/forgejo.db

[repository]
ROOT = $W/data/repositories
DEFAULT_PRIVATE = private
; The first push of a project creates its repository, private.
ENABLE_PUSH_CREATE_USER = true
DEFAULT_PUSH_CREATE_PRIVATE = true

[security]
INSTALL_LOCK = true
SECRET_KEY = $($F generate secret SECRET_KEY)
INTERNAL_TOKEN = $($F generate secret INTERNAL_TOKEN)

[oauth2]
JWT_SECRET = $($F generate secret JWT_SECRET)

[service]
DISABLE_REGISTRATION = true
REQUIRE_SIGNIN_VIEW = true
DEFAULT_KEEP_EMAIL_PRIVATE = true

[log]
ROOT_PATH = $W/log
LEVEL = Warn
EOF
chmod 600 "$W/custom/conf/app.ini"
brew services start forgejo
```

Create your account with a one-time password. Forgejo makes you choose a real
one at first sign-in, and blocks git and API access until you do:

```sh
cd /opt/homebrew/var/forgejo
forgejo admin user create --config custom/conf/app.ini --work-path "$PWD" \
  --username YOU --email you@example.com --admin --must-change-password \
  --password 'temporary-first-sign-in'
```

## 3. Serve it over Tailscale

1. Install Tailscale (`brew install --cask tailscale-app`) and sign in.
2. In the [admin console](https://login.tailscale.com/admin/dns), turn on
   **MagicDNS** and **HTTPS Certificates**.
3. Find this machine's name. It's the `DNSName` in
   `tailscale status --json`, such as `my-mac.tail1234.ts.net`. Then serve
   Forgejo there:
   ```sh
   tailscale serve --bg 3000
   ```
4. Set Forgejo's `ROOT_URL` in `app.ini` to `https://my-mac.tail1234.ts.net/`
   and run `brew services restart forgejo`.

Open that address, sign in, and choose your password. To stop sharing it, run
`tailscale serve --https=443 off`.

## 4. Put a novel on it

1. In Forgejo, go to **Settings → Applications → Generate New Token** and
   give it **repository: read and write**. Copy it, because it's shown only
   once.
2. In the novel's folder (a git repo with at least one commit), point
   `origin` at the server. Put your username in the URL, and keep the token
   in the macOS Keychain:
   ```sh
   git remote add origin "https://YOU@my-mac.tail1234.ts.net/YOU/my-novel.git"
   git config credential.helper osxkeychain
   ```
3. Push once **from the Terminal app**:
   ```sh
   git push -u origin master
   ```
   Paste the token when git asks for a password. The push creates the
   repository, private, and the Keychain remembers the token. Desktop Quill's
   push, pull, and sync use it from then on, through git's credential helper.

## 5. Connect the iPhone

1. Install **Tailscale** from the App Store, sign in with the same account,
   and allow the VPN configuration. Turn on **VPN On Demand** in its settings
   so it reconnects when needed.
2. Check: open `https://my-mac.tail1234.ts.net` in Safari, ideally on cellular
   with Wi-Fi off. You should see Forgejo's sign-in page.
3. In Quill, go to **Projects → Clone from Git…**, enter the same URL
   (`https://YOU@…/YOU/my-novel.git`), and paste a token. The token can be the
   same one, or a separate "iPhone" token you can revoke on its own.

**Keep the username in the URL.** The app sends it with the token, and Forgejo
takes the token as that user's password. Without it, the app sends a generic
username.

## Backups and upkeep

- Every device holds a full clone with complete history, so losing the server
  loses nothing written. Still back up `/opt/homebrew/var/forgejo`, which
  holds the repositories, the database, and `app.ini` with its secrets, along
  with the rest of the machine.
- `brew upgrade forgejo` updates the server. It migrates its own database on
  start.
- Revoke a lost device's token under **Settings → Applications**, and remove
  the device from your tailnet in the Tailscale admin console.
