#!/usr/bin/env bash
# Briardr on a Linux server, reachable from phones one of two ways ("access modes", one per server):
#   public     behind Caddy at one public HTTPS origin (needs a domain; no VPN)
#   wireguard  only through a self-hosted WireGuard tunnel, at https://10.8.0.1:7443 (no domain, no
#              open web ports; Briardr's own CA)
#
#   scripts/setup-linux.sh check   [--access A] [--domain D]   read-only preflight: what's there, what's missing, blockers
#   scripts/setup-linux.sh install --domain D [options]         public: install and configure everything; safe to run again
#   scripts/setup-linux.sh install --access wireguard [options] wireguard: the same, private
#   scripts/setup-linux.sh status                        services, versions, URL
#   scripts/setup-linux.sh pair                          a one-time pairing code for a new device
#   scripts/setup-linux.sh restart                       restart Briardr only (Herdr and its agents keep running)
#   scripts/setup-linux.sh upgrade [REF]                 update the checkout (fast-forward, or REF), rebuild, restart, verify
#   scripts/setup-linux.sh rollback                      go back to the commit before the last upgrade
#   scripts/setup-linux.sh backup [FILE]                 archive Briardr's data (devices, notes, keys) and deployment files
#   scripts/setup-linux.sh restore FILE                  restore such an archive (the current data is kept aside)
#   scripts/setup-linux.sh diagnose                      why a phone can't connect or HTTPS fails
#   scripts/setup-linux.sh wg-add NAME                   wireguard: add a phone; shows its tunnel config once (QR code)
#   scripts/setup-linux.sh wg-list                       wireguard: phones, addresses, last handshake, traffic
#   scripts/setup-linux.sh wg-remove NAME                wireguard: cut a phone off the tunnel immediately
#
# Install options:
#   --access public|wireguard  how phones reach Briardr (default: public with --domain). Switching later is
#                         supported; devices and data are kept, but phones pair again (the URL changes).
#   --domain D            public: the host name (DNS A/AAAA must point at this server)
#   --preview-origin URL  public: preview slot origins with {n} (default https://p{n}.D); needs DNS for p1…p16.D
#   --no-previews         public: turn previews off
#   --internal-tls        public: certificates from Caddy's local CA instead of Let's Encrypt (test VMs only)
#   --wg-port N           wireguard: the UDP port phones connect to (default 51820)
#   --wg-subnet CIDR      wireguard: the tunnel's private network (default 10.8.0.0/24; the server is .1)
#   --endpoint HOST       wireguard: the address phones dial (default: this server's public IPv4)
#   --agents LIST         also install agent CLIs: claude, codex (comma-separated). Never signs them in.
#   --force-os            continue on an untested distribution or release
#
# Run it as the user Briardr, Herdr and the agents run as (not root), from the Briardr checkout. It asks
# for sudo for system packages, the systemd units, Caddy and WireGuard. It never prints or stores
# credentials (a phone's WireGuard config is shown once, by wg-add, and isn't kept), keeps existing
# devices, notes and keys, backs up every system file it changes, validates configuration before
# applying it, and stops at the first failure with what to do next.
set -Eeuo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../deploy/versions.env
. "$REPO/deploy/versions.env"
SETUP_FORMAT=1
STATE_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/briardr-setup
# The server builds outside the checkout: older commits don't ignore a target directory, and untracked
# files there would block upgrade and rollback ("the checkout has local changes").
CARGO_TARGET=$STATE_DIR/cargo-target
BRIARDR_BIN=$CARGO_TARGET/release/briardr
STATE_FILE=$STATE_DIR/setup.env
BACKUPS=$STATE_DIR/backups
LOCAL_BIN=$HOME/.local/bin
SERVICE_PATH="$LOCAL_BIN:$HOME/.local/share/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
CADDY_SITE=/etc/caddy/briardr.caddy
CADDY_MAIN=/etc/caddy/Caddyfile
UNITS=(briardr-herdr.service briardr.service)
WG_IF=wg0
WG_DIR=/etc/wireguard
WG_CONF=$WG_DIR/$WG_IF.conf
WG_GUARD=$WG_DIR/briardr-$WG_IF.nft # nftables: the tunnel's addresses only through the tunnel
WG_MARK='# Managed by Briardr (scripts/setup-linux.sh).'
STAMP=$(date +%Y%m%d-%H%M%S)

# ---------- output ----------
if [ -t 1 ]; then B=$'\e[1m' G=$'\e[32m' Y=$'\e[33m' R=$'\e[31m' N=$'\e[0m'; else B='' G='' Y='' R='' N=''; fi
CURRENT_STEP=''
BLOCKERS=()
WARNINGS=()
step() { CURRENT_STEP=$1; printf '\n%s==> %s%s\n' "$B" "$1" "$N"; }
ok() { printf '  %s✓%s %s\n' "$G" "$N" "$1"; }
info() { printf '  · %s\n' "$1"; }
warn() { WARNINGS+=("$1"); printf '  %s!%s %s\n' "$Y" "$N" "$1"; }
block() { BLOCKERS+=("$1"); printf '  %s✗%s %s\n' "$R" "$N" "$1"; }
die() { printf '\n%sStopped:%s %s\n' "$R" "$N" "$1" >&2; exit 1; }
on_error() {
  local code=$?
  [ "$BASHPID" = "$$" ] || exit "$code" # report once, from the main shell
  printf '\n%sFailed%s during “%s” (exit %s, line %s).\n' "$R" "$N" "${CURRENT_STEP:-setup}" "$code" "${BASH_LINENO[0]}" >&2
  printf 'Steps before it are complete; nothing after it was changed. Fix the cause and run the same command again.\n' >&2
  printf 'Backups of changed system files are in %s.\n' "$BACKUPS" >&2
  exit "$code"
}
trap on_error ERR

# ---------- saved choices ----------
DOMAIN='' PREVIEW_ORIGIN='' PREVIEWS=on TLS=acme AGENTS='' DATA_DIR='' PREVIOUS_COMMIT='' SAVED_FORMAT=$SETUP_FORMAT
ACCESS='' WG_PORT=51820 WG_SUBNET=10.8.0.0/24 WG_ENDPOINT='' SWITCHED_FROM=''
load_state() {
  [ -f "$STATE_FILE" ] || return 0
  # shellcheck disable=SC1090
  . "$STATE_FILE"
  SAVED_FORMAT=${SETUP_FORMAT_SAVED:-$SETUP_FORMAT}
}
save_state() {
  mkdir -p "$STATE_DIR" && chmod 700 "$STATE_DIR"
  {
    printf 'SETUP_FORMAT_SAVED=%q\n' "$SETUP_FORMAT"
    for v in ACCESS DOMAIN PREVIEW_ORIGIN PREVIEWS TLS AGENTS DATA_DIR PREVIOUS_COMMIT WG_PORT WG_SUBNET WG_ENDPOINT SWITCHED_FROM; do printf '%s=%q\n' "$v" "${!v}"; done
  } >"$STATE_FILE.tmp"
  chmod 600 "$STATE_FILE.tmp" && mv "$STATE_FILE.tmp" "$STATE_FILE"
}

# ---------- tools ----------
have() { command -v "$1" >/dev/null 2>&1; }
mise_bin() { if have mise; then command -v mise; elif [ -x "$LOCAL_BIN/mise" ]; then echo "$LOCAL_BIN/mise"; fi; }
in_repo() { (cd "$REPO" && PATH="$SERVICE_PATH" BRIARDR_DATA_DIR="$DATA_DIR" "$@"); }
briardr() { in_repo "$BRIARDR_BIN" "$@"; }
bun_pin() { sed -n 's/^bun *= *"\(.*\)"/\1/p' "$REPO/mise.toml"; }
rust_pin() { sed -n 's/^rust *= *"\(.*\)"/\1/p' "$REPO/mise.toml"; }
herdr_path() {
  local p=''
  if PATH="$SERVICE_PATH" have herdr; then p=$(PATH="$SERVICE_PATH" command -v herdr); fi
  [ -n "$p" ] || return 0
  # A mise shim is a link to mise itself; services need the real executable.
  if [ "$(basename "$(readlink -f "$p")")" = mise ]; then (cd "$HOME" && "$(mise_bin)" which herdr); else readlink -f "$p"; fi
}
herdr_version() { "$1" --version 2>/dev/null | awk '{print $2}'; }
herdr_running() { "$1" status 2>/dev/null | sed -n '/^server:/,/^$/p' | grep -q 'status: running'; }
pkg_installed() { dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q 'install ok installed'; }
sudo_ok() { sudo -n true 2>/dev/null; }
public_ip() { curl -4 -fsS --max-time 5 https://checkip.amazonaws.com 2>/dev/null | tr -d '[:space:]' || true; }
port_listener() { ss -Hltn "sport = :$1" 2>/dev/null | awk 'NR == 1'; }
backup_file() { # back up a (system) file before changing it
  local f=$1 dest
  [ -e "$f" ] || return 0
  dest=$BACKUPS/$STAMP$(dirname "$f")
  mkdir -p "$dest" && chmod 700 "$BACKUPS"
  sudo cat "$f" >"$dest/$(basename "$f")"
  info "backed up $f → $dest/"
}
url_code() { curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$@" 2>/dev/null || true; }

# ---------- WireGuard ----------
ip2int() { local IFS=.; set -- $1; echo $(( ($1 << 24) | ($2 << 16) | ($3 << 8) | $4 )); }
int2ip() { echo "$(( ($1 >> 24) & 255 )).$(( ($1 >> 16) & 255 )).$(( ($1 >> 8) & 255 )).$(( $1 & 255 ))"; }
# Check WG_SUBNET and derive WG_ADDR (the server: the first address), WG_PREFIX, WG_NET and WG_LAST.
wg_subnet() {
  local base=${WG_SUBNET%/*} prefix=${WG_SUBNET#*/} o
  local private='--wg-subnet must be a private IPv4 network (in 10.0.0.0/8, 172.16.0.0/12 or 192.168.0.0/16)'
  [[ $WG_SUBNET =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}/[0-9]{1,2}$ ]] || die "--wg-subnet must be an IPv4 network like 10.8.0.0/24 (got '$WG_SUBNET')"
  for o in ${base//./ }; do [ "$((10#$o))" -le 255 ] || die "--wg-subnet: $base isn't an IPv4 address"; done
  [ "$prefix" -ge 16 ] && [ "$prefix" -le 29 ] || die "--wg-subnet needs a prefix from /16 to /29 (got /$prefix)"
  local n size; n=$(ip2int "$base"); size=$(( 1 << (32 - prefix) ))
  [ $(( n % size )) = 0 ] || die "--wg-subnet: $base isn't the start of a /$prefix network (did you mean $(int2ip $(( n - n % size )))/$prefix?)"
  case $base in
    10.* | 192.168.*) [ "${base#192.168.}" = "$base" ] || [ "$prefix" -ge 16 ] || die "$private" ;;
    172.*) o=${base#172.}; o=${o%%.*}; [ "$o" -ge 16 ] && [ "$o" -le 31 ] || die "$private" ;;
    *) die "$private" ;;
  esac
  WG_NET=$n WG_PREFIX=$prefix WG_ADDR=$(int2ip $(( n + 1 ))) WG_LAST=$(( n + size - 2 ))
}
wg_check_port() { [[ $WG_PORT =~ ^[0-9]{1,5}$ ]] && [ "$WG_PORT" -ge 1 ] && [ "$WG_PORT" -le 65535 ] || die "--wg-port must be a UDP port number (got '$WG_PORT')"; }
briardr_ports() { # Briardr's port and preview ports, from its config: "PORT FIRST LAST"
  local c=$DATA_DIR/config.json
  jq -r '[(.port // 7443), (.previewPortStart // 7444), ((.previewPortStart // 7444) + (.previewPortCount // 16) - 1)] | map(tostring) | join(" ")' "$c" 2>/dev/null || echo '7443 7444 7459'
}
wg_origin() { echo "https://$WG_ADDR:$(briardr_ports | awk '{print $1}')"; }
wg_endpoint() { local ep=${WG_ENDPOINT:-$(public_ip)}; [ -n "$ep" ] || return 1; if [[ $ep == *:* ]]; then echo "[$ep]:$WG_PORT"; else echo "$ep:$WG_PORT"; fi; }

# Root-side edits of the WireGuard configuration. The server's private key and the preshared keys stay
# in root-only files and root processes: never on a command line, in this user's files or in the output,
# except a new phone's preshared key, which wg-add shows once inside that phone's config. wg0.conf is
# also the list of phones: each [Peer] follows a "# briardr-peer: NAME" line.
#   wg_root peers                     NAME ADDRESS PUBLIC-KEY per phone
#   wg_root pubkey                    the server's public key (empty before the first install)
#   wg_root render ADDR/PREFIX PORT   stage wg0.conf from these settings, the current key and phones
#   wg_root add NAME PUBKEY ADDR      stage it with one more phone; prints the new preshared key
#   wg_root remove NAME               stage it without that phone
#   wg_root commit STAMP              validate the staged file and install it: prints changed/unchanged
wg_root() {
  sudo bash -s -- "$WG_CONF" "$WG_GUARD" "$WG_MARK" "$@" <<'ROOT'
set -euo pipefail
umask 077
conf=$1 guard=$2 mark=$3 cmd=$4
shift 4
stage=$(dirname "$conf")/.briardr-staging/$(basename "$conf")
peers() { [ -f "$conf" ] || return 0; awk '/^# briardr-peer: / {f = 1} f' "$conf"; }
key() { [ -f "$conf" ] && sed -n 's/^PrivateKey *= *//p' "$conf" | head -1 || true; }
mkdir -p "$(dirname "$stage")"
case $cmd in
  peers)
    peers | awk '/^# briardr-peer: / {name = $3} /^PublicKey/ {pub = $3} /^AllowedIPs/ {sub(/\/32$/, "", $3); print name, $3, pub}' ;;
  pubkey)
    k=$(key); [ -z "$k" ] || wg pubkey <<<"$k" ;;
  render)
    k=$(key); [ -n "$k" ] || k=$(wg genkey)
    { printf '%s Phones: wg-add, wg-list, wg-remove. Edits by hand are lost.\n' "$mark"
      printf '[Interface]\nAddress = %s\nListenPort = %s\nPrivateKey = %s\n' "$1" "$2" "$k"
      # Piped: Ubuntu's AppArmor profile for wg-quick lets it read /etc/wireguard, but not its nft.
      printf 'PostUp = cat %s | nft -f -\nPostDown = nft delete table inet briardr_wg || true\n' "$guard"
      peers | sed '1{/^$/!s/^/\n/}'
    } >"$stage" ;;
  add)
    peers | grep -qxF "# briardr-peer: $1" && { echo "a phone named $1 exists" >&2; exit 3; }
    peers | grep -q "^AllowedIPs = $3/32$" && { echo "$3 is taken" >&2; exit 3; }
    psk=$(wg genpsk)
    { cat "$conf"; printf '\n# briardr-peer: %s\n[Peer]\nPublicKey = %s\nPresharedKey = %s\nAllowedIPs = %s/32\n' "$1" "$2" "$psk" "$3"; } >"$stage"
    echo "$psk" ;;
  remove)
    peers | grep -qxF "# briardr-peer: $1" || { echo "no phone named $1" >&2; exit 3; }
    awk -v n="# briardr-peer: $1" '/^# briardr-peer: / {skip = ($0 == n)} !skip' "$conf" | cat -s >"$stage" ;;
  commit)
    wg-quick strip "$stage" >/dev/null || { rm -f "$stage"; echo "the new WireGuard configuration didn't validate; nothing was changed" >&2; exit 4; }
    if [ -f "$conf" ] && cmp -s "$stage" "$conf"; then rm -f "$stage"; echo unchanged; exit 0; fi
    result=created
    if [ -f "$conf" ]; then mkdir -p "$(dirname "$conf")/briardr-backups"; cp -p "$conf" "$(dirname "$conf")/briardr-backups/$(basename "$conf").$1"; result=changed; fi
    install -m 0600 "$stage" "$conf"
    rm -f "$stage"
    echo $result ;;
esac
ROOT
}

wg_guard_text() {
  local kp ps pe; read -r kp ps pe < <(briardr_ports)
  local ports="$kp, $ps-$pe"
  [ "$((kp + 1))" = "$ps" ] && ports="$kp-$pe"
  cat <<NFT
# Briardr's WireGuard guard, generated by scripts/setup-linux.sh and loaded by $WG_IF's PostUp.
# Linux accepts packets for any of its addresses on any interface, so the tunnel's addresses are made
# reachable only through $WG_IF; and phones on $WG_IF reach only Briardr (and its previews), nothing else.
table inet briardr_wg
flush table inet briardr_wg
table inet briardr_wg {
	chain input {
		type filter hook input priority filter - 1; policy accept;
		iifname != { "lo", "$WG_IF" } ip daddr $WG_SUBNET drop
		iifname "$WG_IF" ct state established,related accept
		iifname "$WG_IF" tcp dport { $ports } accept
		iifname "$WG_IF" icmp type echo-request accept
		iifname "$WG_IF" drop
	}
}
NFT
}

wg_conf_ours() { ! sudo test -e "$WG_CONF" || [ "$(sudo head -c ${#WG_MARK} "$WG_CONF")" = "$WG_MARK" ]; }

# Install the staged wg0.conf and apply it: phones that stay keep their sessions (wg syncconf).
wg_apply() {
  local r; r=$(wg_root commit "$STAMP")
  local failed="wg-quick@$WG_IF didn't start; see the log above or \`journalctl -u wg-quick@$WG_IF\`"
  sudo systemctl enable -q "wg-quick@$WG_IF"
  if ! systemctl is-active -q "wg-quick@$WG_IF"; then
    sudo systemctl start "wg-quick@$WG_IF" || { sudo journalctl -u "wg-quick@$WG_IF" -n 8 --no-pager | sed 's/^/  · /'; die "$failed"; }
    ok "wg-quick@$WG_IF started"
  elif [ "$r" != unchanged ]; then
    if ! ip -4 -o addr show dev "$WG_IF" | grep -q " $WG_ADDR/$WG_PREFIX "; then
      sudo systemctl restart "wg-quick@$WG_IF" || { sudo journalctl -u "wg-quick@$WG_IF" -n 8 --no-pager | sed 's/^/  · /'; die "$failed"; }
      ok "wg-quick@$WG_IF restarted (its address changed)"
    else
      sudo bash -c "wg syncconf $WG_IF <(wg-quick strip $WG_IF)" || die "couldn't apply $WG_CONF to the running $WG_IF"
      ok "applied to the running $WG_IF (other phones stay connected)"
    fi
  else ok "$WG_CONF unchanged"; fi
  [ "$r" != changed ] || info "previous $WG_CONF kept in $WG_DIR/briardr-backups/ (root only)"
}

setup_wireguard() {
  step "WireGuard: $WG_IF on $WG_ADDR/$WG_PREFIX, UDP $WG_PORT"
  have wg && have wg-quick && have nft || die "wireguard-tools or nftables is missing; run \`scripts/setup-linux.sh install --access wireguard\`"
  wg_conf_ours || die "$WG_CONF exists and wasn't written by this setup. Briardr won't change it; move it away or use another server."
  sudo install -d -m 700 "$WG_DIR"
  local tmp; tmp=$(mktemp -d)
  wg_guard_text >"$tmp/guard.nft"
  local guard_changed=0
  if ! sudo cmp -s "$tmp/guard.nft" "$WG_GUARD"; then
    sudo nft -c -f "$tmp/guard.nft" || die "the generated nftables rules didn't validate; nothing was changed"
    backup_file "$WG_GUARD"
    sudo install -m 0600 "$tmp/guard.nft" "$WG_GUARD"
    guard_changed=1
    ok "wrote $WG_GUARD (tunnel addresses only through $WG_IF; Briardr's ports only)"
  else ok "$WG_GUARD unchanged"; fi
  rm -rf "$tmp"
  local had_key; had_key=$(wg_root pubkey)
  # The tunnel's address can't move under existing phones: their configs would point nowhere.
  if sudo test -f "$WG_CONF"; then
    local cur; cur=$(sudo sed -n 's/^Address *= *//p' "$WG_CONF" | head -1)
    if [ "$cur" != "$WG_ADDR/$WG_PREFIX" ] && [ -n "$(wg_root peers)" ]; then
      die "the tunnel is $cur and has phones. Keep --wg-subnet as it was, or remove the phones first (wg-list, wg-remove)."
    fi
    local port; port=$(sudo sed -n 's/^ListenPort *= *//p' "$WG_CONF" | head -1)
    [ "$port" = "$WG_PORT" ] || [ -z "$(wg_root peers)" ] || warn "the WireGuard port changed from $port to $WG_PORT: each phone needs its config again (wg-remove, then wg-add)"
  fi
  wg_root render "$WG_ADDR/$WG_PREFIX" "$WG_PORT"
  wg_apply
  if [ -n "$had_key" ]; then ok "server key kept (public key $had_key)"; else ok "server key generated (public key $(wg_root pubkey))"; fi
  if [ $guard_changed = 1 ] && ip link show "$WG_IF" >/dev/null 2>&1; then
    sudo nft -f "$WG_GUARD" || die "couldn't load $WG_GUARD"
    ok "guard rules active"
  fi
  local n; n=$(wg_root peers | grep -c . || true)
  info "$n phone(s) on the tunnel"
}

need_wireguard() {
  need_installed
  [ "$ACCESS" = wireguard ] || die "this server uses public access; phones on WireGuard need \`install --access wireguard\` first"
}

wg_add() {
  need_wireguard
  local name=${1:-}
  [[ $name =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,31}$ ]] || die "usage: scripts/setup-linux.sh wg-add NAME (a name for the phone: letters, digits, . _ -)"
  wg_conf_ours && sudo test -f "$WG_CONF" || die "$WG_CONF is missing; run \`scripts/setup-linux.sh install --access wireguard\`"
  local peers; peers=$(wg_root peers)
  awk -v n="$name" '$1 == n {f = 1} END {exit !f}' <<<"$peers" && die "a phone named $name is on the tunnel already. Pick another name, or \`wg-remove $name\` first."
  local i addr=''
  for ((i = WG_NET + 2; i <= WG_LAST; i++)); do
    awk -v a="$(int2ip $i)" '$2 == a {f = 1} END {exit !f}' <<<"$peers" || { addr=$(int2ip $i); break; }
  done
  [ -n "$addr" ] || die "no free address left in $WG_SUBNET"
  local endpoint; endpoint=$(wg_endpoint) || die "couldn't find this server's public IP. Pass it: wg-add $name --endpoint <static IP>"
  local priv pub psk spub client
  priv=$(wg genkey)
  pub=$(wg pubkey <<<"$priv")
  spub=$(wg_root pubkey)
  [ -n "$spub" ] || die "the server has no WireGuard key yet; run install first"
  step "Add $name ($addr)"
  psk=$(wg_root add "$name" "$pub" "$addr")
  wg_apply
  save_state
  client=$(printf '[Interface]\nPrivateKey = %s\nAddress = %s/32\n\n[Peer]\nPublicKey = %s\nPresharedKey = %s\nEndpoint = %s\nAllowedIPs = %s\nPersistentKeepalive = 25\n' "$priv" "$addr" "$spub" "$psk" "$endpoint" "$WG_SUBNET")
  unset priv psk
  printf '\n%sScan this in the WireGuard app on %s (+ → Create from QR code):%s\n\n' "$B" "$name" "$N"
  qrencode -t ansiutf8 -o - <<<"$client"
  printf '\nThe same as text (a private key: only for that device; it is shown once and not stored here):\n\n'
  printf -- '----- BEGIN %s.conf -----\n%s\n----- END %s.conf -----\n' "$name" "$client" "$name"
  unset client
  print_phone_steps
}

print_phone_steps() {
  local origin; origin=$(wg_origin)
  printf '\n%sOn the phone:%s\n' "$B" "$N"
  printf '  1. Install “WireGuard” from the App Store, tap + → Create from QR code, scan the code above.\n'
  printf '     In the tunnel’s settings turn on On-Demand for Cellular and Wi-Fi, then turn the tunnel on.\n'
  printf '  2. In Safari open %s/api/ca.mobileconfig (accept the certificate warning this once)\n' "$origin"
  printf '     and install the profile: Settings → General → VPN & Device Management → Briardr → Install.\n'
  printf '  3. Settings → General → About → Certificate Trust Settings: turn on full trust for “Briardr Local CA”.\n'
  printf '  4. Run %s/scripts/setup-linux.sh pair here, open %s on the phone and enter the code.\n' "$REPO" "$origin"
  printf '  5. Share → Add to Home Screen. Notifications work from the Home Screen app.\n'
  printf '\nClear this terminal’s scrollback when the phone is set up; the key above is the only copy.\n'
}

wg_list() {
  need_wireguard
  local peers; peers=$(wg_root peers)
  [ -n "$peers" ] || { info "no phones yet: scripts/setup-linux.sh wg-add NAME"; return 0; }
  local hs tx now; now=$(date +%s)
  hs=$(sudo wg show "$WG_IF" latest-handshakes 2>/dev/null || true)
  tx=$(sudo wg show "$WG_IF" transfer 2>/dev/null || true)
  printf '%-20s %-15s %-22s %s\n' NAME ADDRESS 'LAST HANDSHAKE' 'RECEIVED / SENT'
  local name addr pub t rx sent ago
  while read -r name addr pub; do
    t=$(awk -v k="$pub" '$1 == k {print $2}' <<<"$hs"); rx=$(awk -v k="$pub" '$1 == k {print $2}' <<<"$tx"); sent=$(awk -v k="$pub" '$1 == k {print $3}' <<<"$tx")
    if [ -z "$t" ]; then ago='not loaded (wg0 down?)'
    elif [ "$t" = 0 ]; then ago=never
    else ago="$(( (now - t) / 60 )) min ago"; [ $(( now - t )) -lt 60 ] && ago="$(( now - t )) s ago"; fi
    printf '%-20s %-15s %-22s %s / %s\n' "$name" "$addr" "$ago" "$(numfmt --to=iec "${rx:-0}")" "$(numfmt --to=iec "${sent:-0}")"
  done <<<"$peers"
}

wg_remove() {
  need_wireguard
  local name=${1:-}
  [ -n "$name" ] || die "usage: scripts/setup-linux.sh wg-remove NAME (see wg-list)"
  awk -v n="$name" '$1 == n {f = 1} END {exit !f}' <<<"$(wg_root peers)" || die "no phone named $name (see wg-list)"
  step "Remove $name"
  wg_root remove "$name"
  wg_apply
  ok "$name is off the tunnel now; its config no longer connects"
  info "Its Briardr sign-in is separate: revoke that device in Settings → Devices (or with \`briardr devices\` and \`briardr revoke\`)."
}

# ---------- check ----------
check() {
  step "Host"
  local id='' ver='' arch
  if [ -r /etc/os-release ]; then id=$(. /etc/os-release && echo "${ID:-}"); ver=$(. /etc/os-release && echo "${VERSION_ID:-}"); fi
  if [ "$id" = ubuntu ] && [[ " $SUPPORTED_UBUNTU " == *" $ver "* ]]; then ok "Ubuntu $ver"
  elif [ "${FORCE_OS:-}" = 1 ]; then warn "${id:-unknown} ${ver} is untested (continuing because of --force-os)"
  else block "${id:-unknown} ${ver}: tested on Ubuntu ${SUPPORTED_UBUNTU// / and }. Other systems need adaptation (or --force-os)."; fi
  arch=$(uname -m)
  case $arch in x86_64 | aarch64) ok "$arch" ;; *) block "CPU architecture $arch isn't supported (x86_64 or aarch64)" ;; esac
  if [ -d /run/systemd/system ]; then ok "systemd"; else block "systemd isn't running (it starts Briardr and Herdr at boot)"; fi
  if [ "$(id -u)" = 0 ]; then block "Run this as the user Briardr runs as, not root (e.g. sudo -iu briardr)"; else ok "user $(id -un) (home $HOME)"; fi
  if ! have sudo; then block "sudo isn't installed"; elif sudo_ok; then ok "sudo"; else info "sudo will ask for your password"; fi
  local free; free=$(df -Pk "$HOME" | awk 'NR==2 {print int($4/1024/1024)}')
  # The Rust toolchain and the server build take about 3 GB.
  if [ "$free" -lt 3 ]; then block "${free} GB free in $HOME (needs at least 5 GB)"; elif [ "$free" -lt 5 ]; then warn "only ${free} GB free in $HOME"; else ok "${free} GB free"; fi

  step "Briardr checkout"
  if [ -f "$REPO/server/Cargo.toml" ] && grep -q "systemd-units" "$REPO/server/src/main.rs"; then ok "$REPO ($(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo 'not a git checkout'))"
  else block "This checkout predates the Rust server (no server/Cargo.toml). Update it first."; fi
  if [ "$SAVED_FORMAT" -gt "$SETUP_FORMAT" ]; then block "$STATE_FILE was written by a newer setup (format $SAVED_FORMAT). Update the checkout before running this one."; fi
  if [ -n "$(git -C "$REPO" status --porcelain 2>/dev/null)" ]; then warn "the checkout has local changes (kept as they are)"; fi

  step "Tools"
  local t
  for t in git curl openssl jq; do if have "$t"; then ok "$t"; else info "$t: will be installed"; fi; done
  if [ "$ACCESS" = wireguard ]; then
    for t in wg qrencode nft; do if have "$t"; then ok "$t"; else info "$t: will be installed"; fi; done
  elif have caddy; then ok "caddy $(caddy version 2>/dev/null | awk '{print $1}')"; else info "caddy: will be installed from Caddy's apt repository"; fi
  local mise; mise=$(mise_bin)
  if [ -n "$mise" ]; then ok "mise $("$mise" --version 2>/dev/null | awk '{print $1}')"; else info "mise: will be installed in $LOCAL_BIN"; fi
  local bunv=''
  if [ -n "$mise" ]; then bunv=$(in_repo "$mise" exec -- bun --version 2>/dev/null || true); fi
  if [ "$bunv" = "$(bun_pin)" ]; then ok "bun $bunv (pinned in mise.toml)"; else info "bun $(bun_pin): will be installed with mise"; fi
  local rustv=''
  if [ -n "$mise" ]; then rustv=$(in_repo "$mise" exec -- rustc --version 2>/dev/null | awk '{print $2}' || true); fi
  if [ "$rustv" = "$(rust_pin)" ]; then ok "rust $rustv (pinned in mise.toml)"; else info "rust $(rust_pin): will be installed with mise (builds the server)"; fi
  local h; h=$(herdr_path)
  if [ -z "$h" ]; then info "herdr $HERDR_VERSION: will be installed in $LOCAL_BIN"
  else
    local hv; hv=$(herdr_version "$h")
    if [ "$hv" = "$HERDR_VERSION" ]; then ok "herdr $hv ($h)"; else warn "herdr $hv at $h; the tested version is $HERDR_VERSION. Left as it is."; fi
    if herdr_running "$h"; then ok "Herdr server running"; else info "Herdr server not running (the briardr-herdr service starts it)"; fi
  fi

  step "Briardr state"
  if [ -d "$DATA_DIR" ]; then
    local mode owner; mode=$(stat -c %a "$DATA_DIR"); owner=$(stat -c %U "$DATA_DIR")
    if [ "$owner" != "$(id -un)" ]; then block "$DATA_DIR belongs to $owner, not $(id -un)"; elif [ "$mode" != 700 ]; then warn "$DATA_DIR has mode $mode (should be 700)"; else ok "$DATA_DIR"; fi
    [ -f "$DATA_DIR/secrets/devices.json" ] && ok "$(jq length "$DATA_DIR/secrets/devices.json" 2>/dev/null || echo '?') paired device(s) (kept)"
  else info "$DATA_DIR: will be created (mode 700)"; fi
  if [ -n "$mise" ] && [ "$bunv" = "$(bun_pin)" ] && [ -f "$DATA_DIR/config.json" ] && [ -d "$REPO/node_modules" ]; then
    local st; st=$(briardr status 2>/dev/null || true)
    local problem; problem=$(echo "$st" | jq -r '.configProblem // empty' 2>/dev/null || true)
    if [ -n "$problem" ]; then block "Briardr's config: $problem"; else ok "config: $(echo "$st" | jq -r '.mode + " " + (.publicOrigin // "")' 2>/dev/null)"; fi
  fi

  step "Services and ports"
  local u
  for u in "${UNITS[@]}" caddy.service "wg-quick@$WG_IF.service"; do
    if systemctl cat "$u" >/dev/null 2>&1; then info "$u: $(systemctl is-enabled "$u" 2>/dev/null || true), $(systemctl is-active "$u" 2>/dev/null || true)"; else info "$u: not installed"; fi
  done
  if [ "$ACCESS" != wireguard ]; then
    for p in 80 443; do
      local l; l=$(port_listener "$p")
      if [ -z "$l" ]; then ok "port $p free"
      elif systemctl is-active -q caddy 2>/dev/null; then ok "port $p: Caddy"
      else block "port $p is used by another program; Caddy needs 80 and 443"; fi
    done
  elif [ -e "$CADDY_SITE" ]; then info "Briardr's public Caddy site will be removed (switching to WireGuard; other Caddy sites stay)"; fi
  local kp; kp=$(jq -r '.port // 7443' "$DATA_DIR/config.json" 2>/dev/null || echo 7443)
  if [ -n "$(port_listener "$kp")" ] && ! systemctl is-active -q briardr 2>/dev/null; then warn "port $kp is in use by something other than the briardr service"; fi
  if have ufw && sudo_ok && sudo ufw status 2>/dev/null | grep -q 'Status: active'; then
    if [ "$ACCESS" = wireguard ]; then info "ufw is active (install allows UDP $WG_PORT, and Briardr's ports on $WG_IF only)"; else info "ufw is active (80/443 are opened by install)"; fi
  fi

  if [ "$ACCESS" = wireguard ]; then
    step "WireGuard"
    if [ -d /sys/module/wireguard ] || modinfo wireguard >/dev/null 2>&1; then ok "kernel module"
    else block "this kernel has no WireGuard module (Linux 5.6 or newer with wireguard; on Ubuntu the standard kernels have it)"; fi
    if ! sudo_ok; then info "$WG_CONF: checked during install (needs sudo)"
    elif sudo test -e "$WG_CONF"; then
      if wg_conf_ours; then ok "$WG_CONF (from this setup; key and phones kept)"; else block "$WG_CONF exists and wasn't written by this setup. Briardr won't change it; move it away or use another server."; fi
    else info "$WG_CONF: will be created with a new server key"; fi
    local udp; udp=$(ss -Hlun "sport = :$WG_PORT" 2>/dev/null | awk 'NR == 1')
    if [ -z "$udp" ]; then ok "UDP $WG_PORT free"
    elif ip link show "$WG_IF" >/dev/null 2>&1 && sudo_ok && [ "$(sudo wg show "$WG_IF" listen-port 2>/dev/null)" = "$WG_PORT" ]; then ok "UDP $WG_PORT: $WG_IF"
    else block "UDP port $WG_PORT is in use by another program; choose another with --wg-port"; fi
    local route; route=$(ip -4 route show to match "$WG_ADDR" 2>/dev/null | grep -v "dev $WG_IF" | grep -v '^default' || true)
    if [ -n "$route" ]; then block "$WG_SUBNET overlaps a network this server already uses ($route); choose another with --wg-subnet"; else ok "tunnel network $WG_SUBNET (server $WG_ADDR)"; fi
    local ep; if ep=$(wg_endpoint); then ok "phones will connect to $ep"; else warn "couldn't find this server's public IPv4; pass --endpoint <static IP>"; fi
    info "The provider's firewall must allow UDP $WG_PORT in (Lightsail: Networking → IPv4 firewall → Custom, UDP, $WG_PORT)."
  fi

  if [ "$ACCESS" != wireguard ] && [ -n "$DOMAIN" ]; then
    step "DNS for $DOMAIN"
    local ip a4 a6; ip=$(public_ip); a4=$(getent ahostsv4 "$DOMAIN" 2>/dev/null | awk 'NR==1{print $1}'); a6=$(getent ahostsv6 "$DOMAIN" 2>/dev/null | awk '$1 !~ /^::ffff:/ {print $1; exit}')
    if [ -z "$a4" ] && [ -z "$a6" ]; then warn "$DOMAIN doesn't resolve yet. Point an A record at ${ip:-the server’s public IP}."
    elif [ -n "$ip" ] && [ -n "$a4" ] && [ "$a4" != "$ip" ]; then warn "$DOMAIN → $a4, but this server's public IPv4 is $ip"
    else ok "$DOMAIN → ${a4:-} ${a6:-}"; fi
    if [ -n "$a6" ] && ! ip -6 addr show scope global 2>/dev/null | grep -q inet6; then warn "$DOMAIN has an AAAA record but this server has no public IPv6: phones on IPv6 networks will fail. Remove the AAAA record or enable IPv6."; fi
    if [ "$PREVIEWS" = on ] && [ -n "$PREVIEW_ORIGIN" ]; then
      local p1=${PREVIEW_ORIGIN//\{n\}/1}
      p1=${p1#https://}
      if getent ahosts "$p1" >/dev/null 2>&1; then ok "previews: $p1 resolves"; else warn "previews: $p1 doesn't resolve. Add a wildcard record *.$DOMAIN (or p1…p16.$DOMAIN)."; fi
    fi
  fi

  printf '\n'
  if [ ${#BLOCKERS[@]} -gt 0 ]; then printf '%s%d blocker(s):%s\n' "$R" "${#BLOCKERS[@]}" "$N"; printf '  - %s\n' "${BLOCKERS[@]}"; return 1; fi
  printf '%sReady.%s %s\n' "$G" "$N" "$([ ${#WARNINGS[@]} -gt 0 ] && echo "${#WARNINGS[@]} warning(s) above." || true)"
}

# ---------- install steps ----------
install_packages() {
  step "System packages"
  local need=() p
  # build-essential: the C compiler and linker the server build needs (Rust links with cc; ring compiles C).
  local pkgs=(git ca-certificates curl openssl unzip jq gnupg build-essential)
  [ "$ACCESS" = wireguard ] && pkgs+=(wireguard-tools qrencode nftables)
  for p in "${pkgs[@]}"; do pkg_installed "$p" || need+=("$p"); done
  if [ ${#need[@]} -gt 0 ]; then
    sudo apt-get update -q
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${need[@]}"
    ok "installed ${need[*]}"
  else ok "already installed"; fi
}

install_caddy() {
  step "Caddy"
  if pkg_installed caddy; then ok "caddy $(caddy version | awk '{print $1}') (installed)"; return; fi
  # Caddy's apt repository keeps it updated: https://caddyserver.com/docs/install#debian-ubuntu-raspbian
  local repo=https://dl.cloudsmith.io/public/caddy/stable list=/etc/apt/sources.list.d/caddy-stable.list
  if curl -fs -o /dev/null --max-time 15 "$repo/deb/debian/dists/any-version/InRelease"; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q debian-keyring debian-archive-keyring apt-transport-https curl
    curl -1sLf "$repo/gpg.key" | sudo gpg --batch --yes --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf "$repo/debian.deb.txt" | sudo tee "$list" >/dev/null
    sudo chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg "$list"
    if sudo apt-get update -q && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q caddy; then
      ok "caddy $(caddy version | awk '{print $1}') from Caddy's apt repository"
      return
    fi
    sudo rm -f "$list" # never leave apt with a source it can't read
    sudo apt-get update -q >/dev/null 2>&1 || true
  fi
  # Fallback: the pinned release package from GitHub, checked against its pinned checksum.
  local arch sum tmp
  case $(uname -m) in x86_64) arch=amd64 sum=$CADDY_SHA256_AMD64 ;; aarch64) arch=arm64 sum=$CADDY_SHA256_ARM64 ;; esac
  tmp=$(mktemp -d)
  curl -fsSL "https://github.com/caddyserver/caddy/releases/download/v$CADDY_VERSION/caddy_${CADDY_VERSION}_linux_$arch.deb" -o "$tmp/caddy.deb"
  echo "$sum  $tmp/caddy.deb" | sha256sum -c --quiet - || die "the downloaded Caddy package doesn't match its pinned checksum"
  chmod 755 "$tmp" && chmod 644 "$tmp/caddy.deb"
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$tmp/caddy.deb"
  rm -rf "$tmp"
  ok "caddy $(caddy version | awk '{print $1}') (pinned release package; Caddy's apt repository wasn't reachable)"
  warn "Caddy came from the pinned release package, so apt won't update it. Rerun install after a Briardr update that raises CADDY_VERSION, or switch to Caddy's apt repository when it's reachable."
}

install_mise_bun() {
  step "mise, Bun and Rust"
  if [ -z "$(mise_bin)" ]; then
    # https://mise.jdx.dev/getting-started.html (per-user installer into ~/.local/bin)
    local tmp; tmp=$(mktemp -d)
    curl -fsSL https://mise.run -o "$tmp/install.sh"
    MISE_INSTALL_PATH=$LOCAL_BIN/mise sh "$tmp/install.sh" >/dev/null
    rm -rf "$tmp"
    ok "mise $("$LOCAL_BIN/mise" --version | awk '{print $1}')"
  else ok "mise (installed)"; fi
  local mise; mise=$(mise_bin)
  in_repo "$mise" trust -q "$REPO/mise.toml"
  in_repo "$mise" install -q
  ok "bun $(in_repo "$mise" exec -- bun --version) (pinned $(bun_pin))"
  ok "rust $(in_repo "$mise" exec -- rustc --version | awk '{print $2}') (pinned $(rust_pin))"
}

build_app() {
  step "Briardr server, dependencies and web client"
  local mise; mise=$(mise_bin)
  mkdir -p "$STATE_DIR"
  # Servers set up before the Rust server have no C compiler yet (install_packages doesn't run on upgrade).
  if ! pkg_installed build-essential; then
    { sudo apt-get update -q && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q build-essential; } >"$STATE_DIR/apt.log" 2>&1 || die "build-essential didn't install (log: $STATE_DIR/apt.log)"
    ok "installed build-essential (the server build needs a C compiler)"
  fi
  # The running server keeps its own copy of the old binary until the restart; a failed build stops here.
  in_repo env CARGO_TARGET_DIR="$CARGO_TARGET" "$mise" exec -- cargo build --release --locked --manifest-path server/Cargo.toml >"$STATE_DIR/server-build.log" 2>&1 || { tail -5 "$STATE_DIR/server-build.log" | sed 's/^/  · /'; die "the server didn't build (full log: $STATE_DIR/server-build.log); the running server is unchanged"; }
  ok "server built ($BRIARDR_BIN)"
  in_repo "$mise" exec -- bun install --frozen-lockfile >/dev/null || die "bun install failed with the committed lockfile. Don't regenerate it on the server; check the checkout and Bun's version."
  ok "dependencies (frozen lockfile)"
  # Build beside the live client and swap only a complete build in: a failed build must not take
  # down the client the running Briardr serves.
  local next=$STATE_DIR/web-next prev=$STATE_DIR/web-prev dist=$REPO/web/dist
  mkdir -p "$STATE_DIR"
  rm -rf "$next"
  in_repo env BRIARDR_WEB_DIST="$next" "$mise" exec -- bun scripts/build.ts >"$STATE_DIR/build.log" 2>&1 || { tail -5 "$STATE_DIR/build.log" | sed 's/^/  · /'; die "the web client didn't build (full log: $STATE_DIR/build.log); the running client is unchanged"; }
  [ -f "$next/index.html" ] || die "the build produced no index.html; the running client is unchanged"
  rm -rf "$prev"
  [ -d "$dist" ] && mv "$dist" "$prev"
  mv "$next" "$dist"
  ok "web client built ($(sed -n 's/.*name="briardr-version" content="\([^"]*\)".*/\1/p' "$dist/index.html" | awk 'NR == 1'))"
}

install_herdr() {
  step "Herdr"
  local h; h=$(herdr_path)
  if [ -n "$h" ]; then
    local hv; hv=$(herdr_version "$h")
    if [ "$hv" = "$HERDR_VERSION" ]; then ok "herdr $hv ($h)"; else warn "herdr $hv at $h (tested: $HERDR_VERSION). Not changed: upgrading Herdr can restart live agents."; fi
    HERDR=$h
    return
  fi
  local arch sum
  case $(uname -m) in x86_64) arch=x86_64 sum=$HERDR_SHA256_LINUX_X86_64 ;; aarch64) arch=aarch64 sum=$HERDR_SHA256_LINUX_AARCH64 ;; esac
  local tmp; tmp=$(mktemp -d)
  curl -fsSL "https://github.com/herdrdev/herdr/releases/download/v$HERDR_VERSION/herdr-linux-$arch" -o "$tmp/herdr"
  echo "$sum  $tmp/herdr" | sha256sum -c --quiet - || die "the downloaded Herdr doesn't match its pinned checksum"
  mkdir -p "$LOCAL_BIN"
  install -m 0755 "$tmp/herdr" "$LOCAL_BIN/herdr"
  rm -rf "$tmp"
  HERDR=$LOCAL_BIN/herdr
  ok "herdr $(herdr_version "$HERDR") → $HERDR (checksum verified)"
}

install_agents() {
  [ -n "$AGENTS" ] || return 0
  step "Agent CLIs"
  local a mise; mise=$(mise_bin)
  for a in ${AGENTS//,/ }; do
    case $a in
      claude)
        if PATH="$SERVICE_PATH" have claude; then ok "claude (installed)"; else
          # https://docs.anthropic.com/en/docs/claude-code/setup (native installer, into ~/.local/bin)
          local tmp; tmp=$(mktemp -d)
          curl -fsSL https://claude.ai/install.sh -o "$tmp/install.sh"
          bash "$tmp/install.sh" >/dev/null
          rm -rf "$tmp"
          ok "claude installed"
        fi ;;
      codex)
        if PATH="$SERVICE_PATH" have codex; then ok "codex (installed)"; else
          # https://github.com/openai/codex (npm package; Node from mise for this user)
          if ! PATH="$SERVICE_PATH" have npm; then (cd "$HOME" && "$mise" use -g node@lts >/dev/null); fi
          (cd "$HOME" && "$mise" exec node@lts -- npm install -g --silent @openai/codex)
          "$mise" reshim
          ok "codex installed"
        fi ;;
      *) warn "unknown agent CLI '$a' (supported: claude, codex)" ;;
    esac
  done
  info "Sign in as $(id -un) yourself: run $(printf '`%s` ' ${AGENTS//,/ })once each in a terminal here. Credentials stay in your home directory."
}

configure_briardr() {
  step "Briardr configuration"
  mkdir -p "$DATA_DIR" && chmod 700 "$DATA_DIR"
  local args=(configure --mode proxy --public-origin "https://$DOMAIN" --preview-origin "$([ "$PREVIEWS" = on ] && echo "$PREVIEW_ORIGIN" || true)" --herdr-path "$HERDR")
  # WireGuard: Briardr serves HTTPS itself (its own CA) on the tunnel address, to the tunnel's network only.
  [ "$ACCESS" = wireguard ] && args=(configure --mode local-ca --listen "$WG_ADDR" --allowed-networks "$WG_SUBNET" --public-origin "$(wg_origin)" --herdr-path "$HERDR")
  briardr "${args[@]}" --dry-run >/dev/null # validates; stops here on a bad value
  local out; out=$(briardr "${args[@]}")
  if [[ $out == Unchanged* ]]; then ok "unchanged ($DATA_DIR/config.json)"; else ok "${out%%. Restart*}"; RESTART_BRIARDR=1; fi
}

install_units() {
  step "systemd services"
  local tmp; tmp=$(mktemp -d)
  local extra=()
  [ "$ACCESS" = wireguard ] && extra=(--network-unit "wg-quick@$WG_IF.service") # Briardr listens on wg0's address
  BRIARDR_SERVICE_PATH="$SERVICE_PATH" briardr systemd-units "$tmp" "${extra[@]}" >/dev/null
  if ! sudo systemd-analyze verify "$tmp/briardr-herdr.service" "$tmp/briardr.service" >"$tmp/verify.log" 2>&1; then
    grep -F "$tmp" "$tmp/verify.log" | sed 's/^/  · /' || true
    die "the generated systemd units didn't verify; nothing was installed"
  fi
  ok "generated units verify"
  local u changed=0
  for u in "${UNITS[@]}"; do
    if ! sudo cmp -s "$tmp/$u" "/etc/systemd/system/$u" 2>/dev/null; then
      backup_file "/etc/systemd/system/$u"
      sudo install -m 0644 "$tmp/$u" "/etc/systemd/system/$u"
      changed=1
      ok "installed /etc/systemd/system/$u"
    else ok "$u unchanged"; fi
  done
  rm -rf "$tmp"
  if [ $changed = 1 ]; then
    sudo systemctl daemon-reload
    RESTART_BRIARDR=1
  fi
  sudo systemctl enable -q "${UNITS[@]}"
  # Herdr: start it under systemd unless a Herdr server is already running outside it (its agents keep running).
  if systemctl is-active -q briardr-herdr; then ok "briardr-herdr running"
  elif herdr_running "$HERDR"; then warn "a Herdr server is already running outside systemd. Left alone so its agents keep running; when convenient, stop it (\`herdr server stop\`) and run \`sudo systemctl start briardr-herdr\`."
  else sudo systemctl start briardr-herdr && ok "briardr-herdr started"; fi
  if [ "${RESTART_BRIARDR:-0}" = 1 ] || ! systemctl is-active -q briardr; then sudo systemctl restart briardr && ok "briardr (re)started"; else ok "briardr running"; fi
}

caddy_is_stock() { # the Caddyfile the package ships: a :80 site serving /usr/share/caddy
  [ -f "$CADDY_MAIN" ] || return 0
  [ "$(grep -v '^\s*#' "$CADDY_MAIN" | tr -d '[:space:]')" = ':80{root*/usr/share/caddyfile_server}' ]
}

configure_caddy() {
  step "Caddy site for https://$DOMAIN"
  local tmp; tmp=$(mktemp)
  briardr caddyfile $([ "$TLS" = internal ] && echo --internal-tls) >"$tmp"
  local import="import $CADDY_SITE" changed=0 site_backup='' main_backup=''
  if ! sudo cmp -s "$tmp" "$CADDY_SITE" 2>/dev/null; then
    if [ -e "$CADDY_SITE" ]; then backup_file "$CADDY_SITE"; site_backup=$BACKUPS/$STAMP$CADDY_SITE; else site_backup=none; fi
    sudo install -m 0644 "$tmp" "$CADDY_SITE"
    changed=1
    ok "wrote $CADDY_SITE"
  else ok "$CADDY_SITE unchanged"; fi
  rm -f "$tmp"
  if caddy_is_stock; then
    backup_file "$CADDY_MAIN"
    main_backup=$BACKUPS/$STAMP$CADDY_MAIN
    printf '# Sites on this server. Briardr’s is generated by scripts/setup-linux.sh.\n%s\n' "$import" | sudo tee "$CADDY_MAIN" >/dev/null
    changed=1
    ok "replaced the package’s placeholder site with: $import"
  elif ! grep -qxF "$import" "$CADDY_MAIN"; then
    backup_file "$CADDY_MAIN"
    main_backup=$BACKUPS/$STAMP$CADDY_MAIN
    printf '\n%s\n' "$import" | sudo tee -a "$CADDY_MAIN" >/dev/null
    changed=1
    ok "added “$import” to $CADDY_MAIN (other sites kept)"
  fi
  if ! sudo -u caddy env HOME=/var/lib/caddy caddy validate --config "$CADDY_MAIN" --adapter caddyfile >/dev/null 2>"$STATE_DIR/caddy-validate.log"; then
    sed 's/^/  · /' "$STATE_DIR/caddy-validate.log" | tail -5
    if [ $changed = 1 ]; then
      [ -n "$main_backup" ] && sudo install -m 0644 "$main_backup" "$CADDY_MAIN"
      if [ "$site_backup" = none ]; then sudo rm -f "$CADDY_SITE"; elif [ -n "$site_backup" ]; then sudo install -m 0644 "$site_backup" "$CADDY_SITE"; fi
      die "the new Caddy configuration didn't validate; the previous files are back in place and Caddy wasn't reloaded"
    fi
    die "Caddy's configuration doesn't validate (see above); nothing was changed"
  fi
  ok "configuration valid"
  sudo systemctl enable -q caddy
  if ! systemctl is-active -q caddy; then sudo systemctl start caddy && ok "caddy started"
  elif [ $changed = 1 ]; then sudo systemctl reload caddy && ok "caddy reloaded (open connections kept)"; fi
}

# Switching to WireGuard: Briardr's public site goes (backed up); Caddy keeps serving any other sites.
remove_caddy_site() {
  local import="import $CADDY_SITE"
  [ -e "$CADDY_SITE" ] || grep -qxF "$import" "$CADDY_MAIN" 2>/dev/null || return 0
  step "Caddy: remove Briardr's public site"
  local main_backup=''
  if grep -qxF "$import" "$CADDY_MAIN" 2>/dev/null; then
    backup_file "$CADDY_MAIN"
    main_backup=$BACKUPS/$STAMP$CADDY_MAIN
    local tmp; tmp=$(mktemp)
    grep -vxF "$import" "$CADDY_MAIN" >"$tmp" || true
    sudo install -m 0644 "$tmp" "$CADDY_MAIN"
    rm -f "$tmp"
    ok "removed “$import” from $CADDY_MAIN"
  fi
  if [ -e "$CADDY_SITE" ]; then backup_file "$CADDY_SITE"; sudo rm -f "$CADDY_SITE"; ok "removed $CADDY_SITE"; fi
  if [ -z "$(grep -v '^\s*#' "$CADDY_MAIN" 2>/dev/null | tr -d '[:space:]')" ]; then
    sudo systemctl disable -q --now caddy 2>/dev/null || true
    ok "Caddy has no other sites: stopped and disabled (nothing listens on 80 and 443)"
  elif sudo -u caddy env HOME=/var/lib/caddy caddy validate --config "$CADDY_MAIN" --adapter caddyfile >/dev/null 2>"$STATE_DIR/caddy-validate.log"; then
    if systemctl is-active -q caddy; then
      sudo systemctl reload caddy || die "Caddy didn't reload (journalctl -u caddy)"
      ok "Caddy reloaded; its other sites keep running"
    fi
  else
    [ -n "$main_backup" ] && sudo install -m 0644 "$main_backup" "$CADDY_MAIN"
    die "Caddy's configuration doesn't validate without Briardr's site (see $STATE_DIR/caddy-validate.log); $CADDY_MAIN is back as it was"
  fi
}

open_firewall() {
  have ufw || return 0
  sudo ufw status 2>/dev/null | grep -q 'Status: active' || return 0
  step "Firewall (ufw)"
  if [ "$ACCESS" = wireguard ]; then
    local kp ps pe; read -r kp ps pe < <(briardr_ports)
    sudo ufw allow "$WG_PORT/udp" comment 'Briardr WireGuard' >/dev/null
    if [ "$((kp + 1))" = "$ps" ]; then sudo ufw allow in on "$WG_IF" to any port "$kp:$pe" proto tcp comment 'Briardr through WireGuard' >/dev/null
    else
      sudo ufw allow in on "$WG_IF" to any port "$kp" proto tcp comment 'Briardr through WireGuard' >/dev/null
      sudo ufw allow in on "$WG_IF" to any port "$ps:$pe" proto tcp comment 'Briardr previews through WireGuard' >/dev/null
    fi
    ok "UDP $WG_PORT open; Briardr's ports ($kp, $ps–$pe) open on $WG_IF only (SSH rules unchanged)"
    if sudo ufw status 2>/dev/null | grep -qE '^(80|443)(/tcp)? +ALLOW'; then
      warn "ufw still allows TCP 80/443 (from public access?). If nothing else needs them: sudo ufw delete allow 80/tcp; sudo ufw delete allow 443/tcp"
    fi
    return 0
  fi
  sudo ufw allow 80/tcp >/dev/null && sudo ufw allow 443/tcp >/dev/null && ok "80 and 443 open (SSH rules unchanged)"
}

caddy_ca() { echo /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt; }

verify() {
  [ "$ACCESS" = wireguard ] && { verify_wireguard; return; }
  step "Verify"
  local port; port=$(jq -r '.port // 7443' "$DATA_DIR/config.json")
  local i code=''
  for i in $(seq 1 30); do
    code=$(url_code -H "Host: $DOMAIN" "http://127.0.0.1:$port/api/session")
    [ "$code" = 401 ] && break
    sleep 1
  done
  [ "$code" = 401 ] || { sudo journalctl -u briardr -n 20 --no-pager | sed 's/^/  · /'; die "Briardr isn't answering on 127.0.0.1:$port (got ${code:-nothing}); its log is above"; }
  ok "Briardr answers on 127.0.0.1:$port"
  local tlsargs=(--resolve "$DOMAIN:443:127.0.0.1")
  if [ "$TLS" = internal ]; then sudo cat "$(caddy_ca)" >"$STATE_DIR/caddy-local-ca.crt" 2>/dev/null || true; tlsargs+=(--cacert "$STATE_DIR/caddy-local-ca.crt"); fi
  for i in $(seq 1 45); do
    code=$(url_code "${tlsargs[@]}" "https://$DOMAIN/api/session")
    [ "$code" = 401 ] && break
    sleep 2
  done
  if [ "$code" != 401 ]; then
    warn "https://$DOMAIN isn't answering through Caddy with a valid certificate yet (got ${code:-no TLS connection})."
    sudo journalctl -u caddy -n 200 --no-pager 2>/dev/null | grep -iE 'error|challenge|acme|certificate' | tail -6 | sed 's/^/  · /' || true
    info "Run \`scripts/setup-linux.sh diagnose\` once DNS and the firewall are in place."
    return 0
  fi
  ok "https://$DOMAIN serves Briardr with a certificate that verifies"
  code=$(url_code "${tlsargs[@]}" --http1.1 -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "Origin: https://$DOMAIN" "https://$DOMAIN/ws/live?v=1")
  if [ "$code" = 401 ]; then ok "WebSockets reach Briardr through Caddy (and need a signed-in device)"; else warn "WebSocket check returned $code (expected 401 without a device)"; fi
  if [ "$TLS" = acme ]; then
    code=$(url_code "https://$DOMAIN/api/session")
    if [ "$code" = 401 ]; then ok "reachable at https://$DOMAIN through public DNS"; else warn "https://$DOMAIN through public DNS: ${code:-no answer}. Check DNS and the provider's firewall (80/443)."; fi
  fi
}

verify_wireguard() {
  step "Verify"
  if ip -4 -o addr show dev "$WG_IF" 2>/dev/null | grep -q " $WG_ADDR/"; then ok "$WG_IF is up with $WG_ADDR"; else die "$WG_IF isn't up with $WG_ADDR (systemctl status wg-quick@$WG_IF)"; fi
  if sudo nft list table inet briardr_wg >/dev/null 2>&1; then ok "guard rules active (tunnel addresses only through $WG_IF)"; else warn "the nftables guard (table inet briardr_wg) isn't loaded; run install again"; fi
  local port; port=$(briardr_ports | awk '{print $1}')
  local ca=$DATA_DIR/secrets/tls/ca.crt origin; origin=$(wg_origin)
  local i code=''
  for i in $(seq 1 30); do
    code=$(url_code --cacert "$ca" "$origin/api/session")
    [ "$code" = 401 ] && break
    sleep 1
  done
  [ "$code" = 401 ] || { sudo journalctl -u briardr -n 20 --no-pager | sed 's/^/  · /'; die "Briardr isn't answering on $origin (got ${code:-nothing}); its log is above"; }
  ok "$origin serves Briardr with a certificate from Briardr's CA"
  code=$(url_code --cacert "$ca" --http1.1 -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H "Origin: $origin" "$origin/ws/live?v=1")
  if [ "$code" = 401 ]; then ok "the WebSocket route answers (and needs a signed-in device)"; else warn "WebSocket check returned $code (expected 401 without a device)"; fi
  code=$(url_code --cacert "$ca" "$origin/api/ca.mobileconfig")
  if [ "$code" = 200 ]; then ok "the CA profile for phones is served"; else warn "$origin/api/ca.mobileconfig returned $code"; fi
  local a out=0
  for a in $(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1); do
    [ "$a" = "$WG_ADDR" ] && continue
    code=$(url_code -k "https://$a:$port/api/session")
    if [ "$code" != 000 ]; then out=1; warn "Briardr answers on $a:$port (got $code); it should listen on $WG_ADDR and loopback only"; fi
  done
  [ $out = 0 ] && ok "not reachable on this server's other addresses"
  return 0
}

print_next() {
  if [ "$ACCESS" = wireguard ]; then
    local ep; ep=$(wg_endpoint || echo "<this server's public IP>:$WG_PORT")
    printf '\n%sBriardr is set up for WireGuard.%s\n' "$B" "$N"
    printf '  URL:       %s   (only through the tunnel)\n' "$(wg_origin)"
    printf '  Tunnel:    UDP %s, phones dial %s; network %s\n' "$WG_PORT" "$ep" "$WG_SUBNET"
    printf '  Phones:    %s/scripts/setup-linux.sh wg-add NAME   (then wg-list, wg-remove NAME)\n' "$REPO"
    printf '  Pair:      %s/scripts/setup-linux.sh pair\n' "$REPO"
    printf '  Status:    systemctl status briardr briardr-herdr wg-quick@%s\n' "$WG_IF"
    printf '  Logs:      journalctl -u briardr -f\n'
    printf '  Restart:   %s/scripts/setup-linux.sh restart   (Briardr only; agents keep running)\n' "$REPO"
    printf '  Data:      %s   (back it up with: scripts/setup-linux.sh backup)\n' "$DATA_DIR"
    WARNINGS+=("the provider's firewall must allow UDP $WG_PORT in (and nothing else but SSH; close 80/443 if they were open for public access)")
  else
    printf '\n%sBriardr is set up.%s\n' "$B" "$N"
    printf '  URL:       https://%s\n' "$DOMAIN"
    printf '  Pair:      %s/scripts/setup-linux.sh pair   (then open the URL on the phone and enter the code)\n' "$REPO"
    printf '  Status:    systemctl status briardr briardr-herdr caddy\n'
    printf '  Logs:      journalctl -u briardr -f   ·   journalctl -u caddy -f\n'
    printf '  Restart:   %s/scripts/setup-linux.sh restart   (Briardr only; agents keep running)\n' "$REPO"
    printf '  Data:      %s   (back it up with: scripts/setup-linux.sh backup)\n' "$DATA_DIR"
    [ "$PREVIEWS" = on ] && printf '  Previews:  %s (DNS: a wildcard record *.%s)\n' "$PREVIEW_ORIGIN" "$DOMAIN"
    if systemctl is-active -q "wg-quick@$WG_IF" 2>/dev/null; then
      info "WireGuard ($WG_IF) still runs, but Briardr no longer listens on it. Stop it if nothing else uses it: sudo systemctl disable --now wg-quick@$WG_IF"
    fi
  fi
  if [ -n "$SWITCHED_FROM" ]; then
    WARNINGS+=("access changed from $SWITCHED_FROM to $ACCESS, so Briardr's URL changed: pair each phone again (scripts/setup-linux.sh pair), add it to the Home Screen again and delete the old Home Screen app. Old devices stay listed in Settings → Devices; revoke the ones you no longer use.")
  fi
  [ ${#WARNINGS[@]} -gt 0 ] && { printf '\n%sStill to do:%s\n' "$Y" "$N"; printf '  - %s\n' "${WARNINGS[@]}"; }
  return 0
}

install_all() {
  [ -n "$ACCESS" ] || die "choose how phones reach Briardr: --domain D (public HTTPS) or --access wireguard (private, no domain)"
  if [ "$ACCESS" = public ]; then
    [ -n "$DOMAIN" ] || die "--domain is needed for public access (the host name phones will open)"
    [[ $DOMAIN =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ]] && [[ $DOMAIN == *.* ]] || die "--domain must be a host name like briardr.example.com"
    [ "$PREVIEWS" = on ] && [ -z "$PREVIEW_ORIGIN" ] && PREVIEW_ORIGIN="https://p{n}.$DOMAIN"
  fi
  if ! check; then die "fix the blockers above, then run install again"; fi
  WARNINGS=()
  # Remembered until an install finishes, so a switch that stops halfway still ends with its advice.
  [ -z "$SAVED_ACCESS" ] || [ "$SAVED_ACCESS" = "$ACCESS" ] || SWITCHED_FROM=$SAVED_ACCESS
  save_state
  install_packages
  [ "$ACCESS" = public ] && install_caddy
  install_mise_bun
  build_app
  install_herdr
  install_agents
  configure_briardr
  if [ "$ACCESS" = wireguard ]; then
    setup_wireguard # before Briardr restarts: it listens on the tunnel's address
    install_units
    remove_caddy_site
  else
    install_units
    configure_caddy
  fi
  open_firewall
  verify
  print_next
  SWITCHED_FROM=''
  save_state
}

# ---------- day-to-day ----------
need_installed() { [ -n "$ACCESS" ] && [ -f "$DATA_DIR/config.json" ] || die "Briardr isn't set up here yet: run \`scripts/setup-linux.sh install --domain …\` (or \`install --access wireguard\`) first"; }

status() {
  need_installed
  step "Services"
  local front=caddy; [ "$ACCESS" = wireguard ] && front="wg-quick@$WG_IF"
  systemctl --no-pager --lines=0 status "${UNITS[@]}" "$front" 2>/dev/null | grep -E '^(●|○|×)|Active:' | sed 's/^/  /' || true
  step "Briardr"
  briardr status | jq '{mode, publicOrigin, previewOrigins, listen, listenMissing, dataDir, herdr: .herdr.running, herdrVersion: .herdr.version, webBuilt}'
  info "access: $ACCESS"
  info "commit $(git -C "$REPO" rev-parse --short HEAD) on $(git -C "$REPO" describe --tags --exact-match 2>/dev/null || git -C "$REPO" rev-parse --abbrev-ref HEAD)"
  if [ "$ACCESS" = wireguard ]; then step "Phones on WireGuard"; wg_list; fi
}

pair() {
  need_installed
  briardr pair
}

restart_briardr() {
  need_installed
  step "Restart Briardr"
  sudo systemctl restart briardr
  verify
}

rebuild_and_restart() { # after the checkout moved: deps, build, deployment files, restart, verify
  install_mise_bun
  build_app
  HERDR=$(jq -r .herdrPath "$DATA_DIR/config.json")
  briardr status >/dev/null || die "the new version doesn't accept the current configuration (see \`briardr status\`)"
  RESTART_BRIARDR=1
  if [ "$ACCESS" = wireguard ]; then
    setup_wireguard
    install_units
  else
    install_units
    configure_caddy
  fi
  verify
}

# WireGuard access needs a setup script that knows it; older versions would redeploy as public access.
supports_access() {
  [ "$ACCESS" = wireguard ] || return 0
  git -C "$REPO" show "$1:scripts/setup-linux.sh" 2>/dev/null | grep -q '^ *wg-add)' || die "$(git -C "$REPO" rev-parse --short "$1" 2>/dev/null || echo "$1") predates WireGuard access in scripts/setup-linux.sh; this server uses it, so that version can't be deployed here"
}

upgrade() {
  need_installed
  local ref=${1:-}
  step "Update the checkout"
  [ -z "$(git -C "$REPO" status --porcelain)" ] || die "the checkout has local changes; commit or stash them first"
  local prev; prev=$(git -C "$REPO" rev-parse HEAD)
  git -C "$REPO" fetch --tags --quiet
  local target='@{upstream}'
  [ -z "$ref" ] || target=$ref
  supports_access "$target"
  if [ -n "$ref" ]; then git -C "$REPO" -c advice.detachedHead=false checkout --quiet "$ref"
  else git -C "$REPO" merge --ff-only --quiet '@{upstream}' || die "can't fast-forward to the upstream branch; pass the commit or tag to deploy"; fi
  if [ "$(git -C "$REPO" rev-parse HEAD)" = "$prev" ]; then ok "already at $(git -C "$REPO" rev-parse --short HEAD)"; else ok "$(git -C "$REPO" rev-parse --short "$prev") → $(git -C "$REPO" rev-parse --short HEAD)"; fi
  PREVIOUS_COMMIT=$prev
  save_state
  # The rest is the new version's job: its build, units and Caddy site may differ from this one's.
  hand_over "$prev"
}

# Continue with the checked-out version's script when it knows how; older versions don't.
hand_over() {
  if grep -q '^ *after-checkout)' "$REPO/scripts/setup-linux.sh"; then exec bash "$REPO/scripts/setup-linux.sh" after-checkout "$1"; fi
  after_checkout "$1"
}

# After upgrade or rollback moved the checkout (run by the new version): rebuild, redeploy, verify;
# on failure, go back to the commit it came from.
after_checkout() {
  local prev=$1
  trap - ERR
  set +e
  (set -e; trap on_error ERR; rebuild_and_restart)
  local rc=$?
  set -e
  trap on_error ERR
  if [ $rc != 0 ]; then
    printf '\n%sThe update failed; going back to %s.%s\n' "$Y" "$(git -C "$REPO" rev-parse --short "$prev")" "$N"
    git -C "$REPO" -c advice.detachedHead=false checkout --quiet "$prev"
    exec bash "$REPO/scripts/setup-linux.sh" after-failed-update
  fi
  info "Roll back with: scripts/setup-linux.sh rollback"
}

rollback() {
  need_installed
  [ -n "$PREVIOUS_COMMIT" ] || die "no earlier commit recorded (rollback works after an upgrade)"
  step "Roll back to $(git -C "$REPO" rev-parse --short "$PREVIOUS_COMMIT")"
  [ -z "$(git -C "$REPO" status --porcelain)" ] || die "the checkout has local changes; commit or stash them first"
  supports_access "$PREVIOUS_COMMIT"
  local cur; cur=$(git -C "$REPO" rev-parse HEAD)
  git -C "$REPO" -c advice.detachedHead=false checkout --quiet "$PREVIOUS_COMMIT"
  ok "now at $(git -C "$REPO" rev-parse --short HEAD) (rollback again returns to $(git -C "$REPO" rev-parse --short "$cur"))"
  PREVIOUS_COMMIT=$cur
  save_state
  hand_over "$cur"
}

backup() {
  need_installed
  local out=${1:-$HOME/briardr-backups/briardr-$STAMP.tar.gz}
  step "Back up"
  mkdir -p "$(dirname "$out")"
  local tmp; tmp=$(mktemp -d)
  mkdir -p "$tmp/deploy"
  [ ! -e "$CADDY_SITE" ] || sudo cat "$CADDY_SITE" >"$tmp/deploy/briardr.caddy"
  for u in "${UNITS[@]}"; do sudo cat "/etc/systemd/system/$u" >"$tmp/deploy/$u" 2>/dev/null || true; done
  cp "$STATE_FILE" "$tmp/deploy/setup.env" 2>/dev/null || true
  if [ "$ACCESS" = wireguard ] && sudo test -f "$WG_CONF"; then
    # Phones' names, addresses and public keys only. The server's private key and the preshared keys stay
    # out on purpose: restoring onto another server gives it a new key, and each phone gets a new config.
    { echo "# server public key: $(wg_root pubkey)"; echo "# name address public-key"; wg_root peers; } >"$tmp/deploy/wireguard-peers.txt"
    sudo cat "$WG_GUARD" >"$tmp/deploy/briardr-$WG_IF.nft" 2>/dev/null || true
  fi
  git -C "$REPO" rev-parse HEAD >"$tmp/deploy/commit"
  (umask 077 && tar -czf "$out" -C "$(dirname "$DATA_DIR")" --exclude="$(basename "$DATA_DIR")/logs" "$(basename "$DATA_DIR")" -C "$tmp" deploy)
  rm -rf "$tmp"
  chmod 600 "$out"
  ok "$out ($(du -h "$out" | cut -f1)). It holds device token hashes, push keys and notes: keep it private (mode 600)."
  info "Agents' sign-ins and your repositories aren't in it; back up the server's disk for those (e.g. Lightsail snapshots)."
  [ "$ACCESS" = wireguard ] && info "WireGuard keys aren't in it either (only the phones' names and public keys): on a new server, add each phone again with wg-add. Disk snapshots keep $WG_DIR."
  return 0
}

restore() {
  need_installed
  local file=${1:-}
  [ -f "$file" ] || die "usage: scripts/setup-linux.sh restore FILE"
  step "Restore $file"
  local name; name=$(basename "$DATA_DIR")
  tar -tzf "$file" | grep -vE "^($name|deploy)(/|$)" | grep -q . && die "$file contains files outside $name/ and deploy/; not restoring it"
  tar -tzf "$file" | grep -q "^$name/config.json$" || die "$file has no $name/config.json"
  sudo systemctl stop briardr
  local aside="$DATA_DIR.before-restore-$STAMP"
  mv "$DATA_DIR" "$aside"
  ok "current data kept at $aside"
  (umask 077 && tar -xzf "$file" -C "$(dirname "$DATA_DIR")" "$name")
  chmod 700 "$DATA_DIR"
  ok "restored $DATA_DIR"
  sudo systemctl start briardr
  verify
  info "Devices paired after the backup need pairing again; devices removed since then are back. Revoke any you don't recognize in Settings → Devices."
}

diagnose() {
  need_installed
  check || true
  [ "$ACCESS" = wireguard ] && { diagnose_wireguard; return; }
  step "HTTPS"
  local cert; cert=$(echo | timeout 10 openssl s_client -connect 127.0.0.1:443 -servername "$DOMAIN" 2>/dev/null | openssl x509 -noout -issuer -enddate 2>/dev/null || true)
  if [ -n "$cert" ]; then echo "$cert" | sed 's/^/  · /'; else warn "Caddy has no certificate for $DOMAIN yet"; fi
  sudo journalctl -u caddy --since '-2h' --no-pager 2>/dev/null | grep -iE 'error|challenge|acme|rate' | tail -8 | sed 's/^/  · /' || true
  info "Let's Encrypt needs port 80 and 443 reachable from the internet and DNS pointing here. On AWS Lightsail/EC2 or Azure, open 80 and 443 in the provider's firewall too."
  step "Briardr"
  sudo journalctl -u briardr -n 15 --no-pager 2>/dev/null | sed 's/^/  · /' || true
  info "If a phone shows “Can’t reach the server”: open https://$DOMAIN in Safari on mobile data and on Wi-Fi. If it loads on one but not the other, the network or DNS on that side is the problem."
}

diagnose_wireguard() {
  step "Tunnel"
  if systemctl is-active -q "wg-quick@$WG_IF"; then ok "wg-quick@$WG_IF active"; else warn "wg-quick@$WG_IF isn't running: sudo systemctl start wg-quick@$WG_IF; journalctl -u wg-quick@$WG_IF"; fi
  if [ -n "$(ss -Hlun "sport = :$WG_PORT" 2>/dev/null)" ]; then ok "listening on UDP $WG_PORT"; else warn "nothing listens on UDP $WG_PORT"; fi
  local ep; ep=$(wg_endpoint || true)
  [ -z "$WG_ENDPOINT" ] || [ "$WG_ENDPOINT" = "$(public_ip)" ] || info "phones dial $WG_ENDPOINT (--endpoint), this server's public IPv4 is $(public_ip || echo unknown)"
  info "phones dial ${ep:-?}: the provider's firewall must allow UDP $WG_PORT in (Lightsail: Networking → IPv4 firewall)"
  wg_list || true
  step "Briardr"
  local origin; origin=$(wg_origin)
  local code; code=$(url_code --cacert "$DATA_DIR/secrets/tls/ca.crt" "$origin/api/session")
  if [ "$code" = 401 ]; then ok "Briardr answers on $origin"; else warn "Briardr isn't answering on $origin (got ${code:-nothing})"; fi
  if sudo nft list table inet briardr_wg >/dev/null 2>&1; then ok "guard rules loaded"; else warn "guard rules (table inet briardr_wg) aren't loaded: sudo systemctl restart wg-quick@$WG_IF"; fi
  sudo journalctl -u briardr -n 15 --no-pager 2>/dev/null | sed 's/^/  · /' || true
  printf '\n%sIf a phone can’t connect:%s\n' "$B" "$N"
  printf '  · Never a handshake (wg-list): the phone doesn’t reach UDP %s. Check the provider firewall, that the\n' "$WG_PORT"
  printf '    tunnel is on in the WireGuard app, and that its Endpoint is %s. A phone whose clock is far off can\n' "${ep:-this server’s IP:$WG_PORT}"
  printf '    be refused: set its time automatically. If the server’s public IP changed, edit the Endpoint on each phone.\n'
  printf '  · A recent handshake but Briardr doesn’t load: open %s in Safari. A certificate warning means the\n' "$origin"
  printf '    Briardr CA isn’t installed or not fully trusted (Settings → General → About → Certificate Trust Settings).\n'
  printf '    “Can’t connect” means Briardr isn’t running (above) or the tunnel’s AllowedIPs don’t include %s.\n' "$WG_ADDR"
  printf '  · Works on Wi-Fi but not cellular (or the reverse): turn on On-Demand for both in the WireGuard app.\n'
}

# ---------- main ----------
# One compound command, read whole before it runs: upgrade replaces this file on disk while it runs.
{
  CMD=${1:-help}
  shift || true
  load_state
  DATA_DIR=${DATA_DIR:-${BRIARDR_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/briardr}}
  [ -z "$ACCESS" ] && [ -n "$DOMAIN" ] && ACCESS=public # set up before access modes existed
  SAVED_ACCESS=$ACCESS ACCESS_ARG='' DOMAIN_ARG=''
  ARGS=()
  while [ $# -gt 0 ]; do
    case $1 in
      --access) ACCESS_ARG=${2:?--access needs a value (public or wireguard)}; shift ;;
      --domain) DOMAIN=${2:?--domain needs a value}; DOMAIN_ARG=1; shift ;;
      --wg-port) WG_PORT=${2:?--wg-port needs a value}; shift ;;
      --wg-subnet) WG_SUBNET=${2:?--wg-subnet needs a value}; shift ;;
      --endpoint) WG_ENDPOINT=${2:?--endpoint needs a value}; shift ;;
      --preview-origin) PREVIEW_ORIGIN=${2:?--preview-origin needs a value}; PREVIEWS=on; shift ;;
      --no-previews) PREVIEWS=off PREVIEW_ORIGIN='' ;;
      --agents) AGENTS=${2:?--agents needs a value}; shift ;;
      --internal-tls) TLS=internal ;;
      --acme-tls) TLS=acme ;;
      --force-os) FORCE_OS=1 ;;
      -h | --help) CMD=help ;;
      -*) die "unknown option $1" ;;
      *) ARGS+=("$1") ;;
    esac
    shift
  done
  case $ACCESS_ARG in
    '') if [ -n "$DOMAIN_ARG" ]; then
          [ "$ACCESS" != wireguard ] || [ "$CMD" != install ] || die "this server uses WireGuard access. To switch it to public HTTPS, add --access public."
          ACCESS=public
        fi ;;
    public | wireguard) ACCESS=$ACCESS_ARG ;;
    *) die "--access is public or wireguard (got '$ACCESS_ARG')" ;;
  esac
  [ "$PREVIEWS" = on ] && [ -n "$DOMAIN" ] && [ -z "$PREVIEW_ORIGIN" ] && PREVIEW_ORIGIN="https://p{n}.$DOMAIN"
  if [ "$ACCESS" = wireguard ]; then wg_subnet; wg_check_port; fi

  case $CMD in
    check) check ;;
    install) install_all ;;
    status) status ;;
    pair) pair ;;
    restart) restart_briardr ;;
    upgrade) upgrade "${ARGS[@]:-}" ;;
    rollback) rollback ;;
    after-checkout) need_installed && after_checkout "${ARGS[0]}" ;;
    after-failed-update) need_installed && rebuild_and_restart && die "the update failed and was rolled back" ;;
    backup) backup "${ARGS[@]:-}" ;;
    restore) restore "${ARGS[@]:-}" ;;
    diagnose) diagnose ;;
    wg-add) wg_add "${ARGS[@]:-}" ;;
    wg-list) wg_list ;;
    wg-remove) wg_remove "${ARGS[@]:-}" ;;
    *) sed -n '2,/^set -Eeuo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//' ;;
  esac
  exit
}
