# notarize-macos

Code-sign + notarize a standalone macOS `fugue`/`fugue-mcp` executable in CI.
This is the single source of truth for macOS release signing; the `fugue-cli`
and `fugue-mcp` release workflows both call it after checking out `gdamron/fugue`
into `./fugue`.

## Why

Unsigned / un-notarized binaries trip macOS Gatekeeper ("cannot be opened
because the developer cannot be verified"). Installed via `curl | sh` there is
no GUI "Open Anyway" escape hatch, so a new Mac user is stuck on first run.
Signing + notarizing removes that wall.

## Usage

```yaml
- name: Check Out Fugue Library
  uses: actions/checkout@v4
  with:
    repository: gdamron/fugue
    path: fugue

# ... build the release binary ...

- name: Sign and notarize (macOS)
  if: matrix.os == 'macos'
  uses: ./fugue/.github/actions/notarize-macos
  with:
    binary-path: fugue-cli/target/${{ matrix.target }}/release/fugue
    certificate-base64: ${{ secrets.MACOS_CERTIFICATE_BASE64 }}
    certificate-password: ${{ secrets.MACOS_CERTIFICATE_PASSWORD }}
    api-key-id: ${{ secrets.MACOS_NOTARY_API_KEY_ID }}
    api-issuer-id: ${{ secrets.MACOS_NOTARY_API_ISSUER_ID }}
    api-key-base64: ${{ secrets.MACOS_NOTARY_API_KEY_BASE64 }}
```

The action signs the binary **in place**, so the existing `tar`/package step
ships the signed artifact unchanged.

## Gated activation

The action is safe to merge before any Apple credentials exist:

- **No secrets** → the step prints a notice and exits 0 (ships unsigned).
- **All secrets present** → it signs + notarizes automatically, no workflow edit.
- **Certificate set but a notary secret missing** → it fails loudly rather than
  shipping a half-signed artifact.

## Required repository secrets

Add these to **each** repo that calls the action (`fugue-cli`, `fugue-mcp`), or
to a shared organization secret set. All values are the raw secret unless noted
as base64.

| Secret | What it is | How to get it |
| --- | --- | --- |
| `MACOS_CERTIFICATE_BASE64` | Developer ID Application cert + private key, exported as `.p12`, then `base64`-encoded | Xcode → Accounts → Manage Certificates → **Developer ID Application** → export to `.p12`; `base64 -i cert.p12 \| pbcopy` |
| `MACOS_CERTIFICATE_PASSWORD` | Password you set when exporting the `.p12` | — |
| `MACOS_NOTARY_API_KEY_ID` | App Store Connect API key ID | App Store Connect → Users and Access → Integrations → Keys |
| `MACOS_NOTARY_API_ISSUER_ID` | Issuer ID for that key | same page (shown above the key list) |
| `MACOS_NOTARY_API_KEY_BASE64` | The `.p8` private key, `base64`-encoded | Download once at key creation; `base64 -i AuthKey_XXXX.p8 \| pbcopy` |

The App Store Connect key needs the **Developer** role (sufficient for
`notarytool`). Prefer a dedicated key so it can be revoked independently.

## Stapling

`xcrun stapler` only staples bundles/installers (`.app`/`.dmg`/`.pkg`), not a
bare command-line executable. A notarized CLI binary shipped in a `.tar.gz`
therefore carries no stapled ticket — Gatekeeper fetches the ticket online on
first run, which is the standard, supported path for CLI tools. Offline
stapling becomes possible once the binaries are wrapped in a `.dmg`/`.pkg`
(tracked under FUG-227).

## Verifying on a clean machine

CI runners are not a clean, quarantined environment, so the definitive
Gatekeeper check is manual, on a Mac that has never seen the binary:

```sh
# Simulate a download (adds the quarantine attribute):
xattr -w com.apple.quarantine "0081;00000000;Safari;" ./fugue
codesign --verify --deep --strict --verbose=2 ./fugue   # signature intact
spctl --assess --type execute --verbose ./fugue         # notarization/online check
./fugue --version                                        # must run with no right-click override
```
