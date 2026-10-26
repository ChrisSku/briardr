# Connect your devices

Briardr is a web app. Install the server once on the machine running Herdr, then connect each client
device to that server. No native Briardr client is required.

## Before pairing

1. Start Briardr and note its HTTPS URL (`briardr status` or `briardr pair`).
2. Connect the client to the correct network: the same Tailscale tailnet, your WireGuard tunnel,
   or the internet for a public HTTPS deployment.
3. Check HTTPS trust. Tailscale certificates and public Caddy certificates normally need no manual
   trust. A local CA must be installed and trusted on each device; see below.

## iPhone and iPad

1. Open the server's URL in Safari.
2. Choose **Share → Add to Home Screen** and add Briardr.
3. Open Briardr from the Home Screen. Pair here: the installed app may have a separate sign-in from
   Safari, so a Safari pairing may not carry over.
4. On the host, run `briardr pair`, or `scripts/setup-linux.sh pair` for a managed Linux install.
5. Enter the one-time code and a device name in Briardr. The code expires after ten minutes.
6. Enable and test notifications in Briardr Settings → Notifications.

If you paired in a browser tab first, Briardr suggests installing it; Settings → Install app has the
steps for each browser. The installed app may then need its own pairing code.

On iPhone and iPad, Web Push requires the Home Screen app. Use a current OS version that supports
Home Screen Web Push (iOS/iPadOS 16.4 or later). Browser automation covers layouts and interactions;
it does not establish real push delivery on a physical device.

The iPad layout makes room for navigation beside a stream. For dictation, the keyboard's built-in
dictation works in text fields. Briardr's separate host-side dictation feature requires a compatible
Mac host; it is unavailable with a Linux host.

## Mac desktop client

Open the HTTPS URL in Safari or another modern browser. In Safari versions supporting web apps,
choose **File → Add to Dock**, then open the Dock app and pair there. A browser tab works too.

The Mac used as a client does not have to be the host. Notification permissions belong to the
browser or installed app in which you enabled them.

## Android, Windows, and Linux desktop clients

Open the URL in a modern browser and pair. If the browser offers **Install app** or **Add to Home
screen**, use it and pair inside that installation. These client combinations need additional
device testing; please report the OS, browser, access mode, and observed behavior when helping test.
Windows hosting is a separate concern and is not currently supported.

## Trust Briardr's local CA

Only do this for a Briardr installation you control. A trusted root CA can sign certificates your
device will accept. Use the CA from your own host and remove it when retiring that installation.

### iPhone or iPad

1. Connect the VPN if applicable. In Safari, open your server URL with
   `/api/ca.mobileconfig` appended, for example `https://10.8.0.1:7443/api/ca.mobileconfig`.
2. The initial visit may warn about the untrusted certificate. Confirm that this is your server
   before proceeding to download the profile.
3. Install the downloaded profile in **Settings → General → VPN & Device Management**.
4. Open **Settings → General → About → Certificate Trust Settings** and enable full trust for
   **Briardr Local CA**. Profile installation alone does not enable TLS trust; see
   [Apple's instructions](https://support.apple.com/en-us/102390).
5. Reopen Briardr and confirm the HTTPS warning is gone before pairing.

### macOS

Copy the public CA certificate from the host's data directory, `secrets/tls/ca.crt`, to the client
using a trusted channel. Import it with Keychain Access into the appropriate keychain, then configure
trust for that certificate. Remove it when it is no longer needed. Never copy `ca.key`.

### Other clients

Import the public `ca.crt` into the OS or browser's trusted certificate store using that platform's
instructions. Some browsers use a separate trust store, and some mobile apps do not trust user-added
CAs. These paths need device testing; publicly trusted HTTPS avoids the manual trust step.

## Test notifications

In Settings → Notifications, choose **Allow notifications**, then **Send test**. On iPhone and iPad,
do this from the Home Screen app. Briardr distinguishes acceptance by the push service from confirmed
delivery to the device. An accepted message alone does not prove it arrived.

Settings → Notifications also says when the permission, this device's subscription, and the host's
record of it disagree, and repairs what it can (Repair renews an expired subscription). Where the
platform supports badges, the app icon shows how many agents need you.

Then close or background the app, trigger an agent needing attention, and check that the alert
arrives and opens the correct stream. Test once on Wi-Fi and once on mobile data with the VPN
connected if needed. Notifications need the host to remain awake and online, and require outbound
access to the browser's push service even when Briardr itself is private.

Real iPhone push and Home Screen behavior on a WireGuard **IP-address origin with a private CA**
are still unverified. The [Linux guide](linux-deployment.md#add-a-phone) records the known checks
and a hostname fallback to try; the fallback itself is not a guarantee of push delivery.

## Offline and updates

The installed app opens without a connection: a launch waits at most 3 seconds for the network (or
a proxy's 502/503/504) before opening the cached app, labeled stale, and reconnects by itself. When
the host gets a new build, open clients install the new offline shell first and then reload. While
an upload runs, a sheet holds unsaved text, or you typed in the terminal in the last 30 seconds, the
reload waits and an "update ready" bar offers it instead.

Settings → Diagnostics shows whether Briardr opens offline and whether the browser keeps its
storage. **Repair offline app** removes the service worker and its caches and installs them again;
notes, drafts, pairing, and settings stay. A device keeps this build's offline shell and the previous
one, the 200 most recent plugin files, and cached terminal screens of streams opened in the last 30
days (at most 100).

## Lost device or changed URL

Revoke a lost device in Settings → Devices, or run `briardr devices` followed by
`briardr revoke DEVICE_ID` on the host. For managed WireGuard, also run
`scripts/setup-linux.sh wg-remove NAME`. Deleting the Home Screen icon does not revoke the device.
If Briardr cannot save the revocation, it reports an error and the device keeps access. Restore write
access to the data directory (or free disk space), then retry the revocation.

Changing the server's hostname, scheme, or port creates a different browser origin. Install again,
pair again, and enable notifications there; remove the old installation and revoke unused devices.
