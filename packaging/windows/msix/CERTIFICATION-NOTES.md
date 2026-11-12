# Atlas File Manager — notes for the next submission

The 0.1.6-beta.1 preparation declares Microsoft.VCLibs.140.00.UWPDesktop
(minimum 14.0.24217.0) in the MSIX manifest. Microsoft Store installs and services
that framework. App-local Visual Studio CRT DLLs are used only by the Inno
test installer; they are not substituted for the framework in the Store MSIX.
This is a packaging correction, not a claim that clean-Windows launch testing
or Microsoft certification has passed. See docs/beta-bugfixes-2026-11-12.md.

Review these notes against the exact submitted binary before copying them into
Partner Center. These are preparation notes, not a record of completed testing.

Atlas is a Windows desktop file manager. The `runFullTrust` capability is used
to execute the desktop application and access the files and folders the user
chooses to browse/manage, including normal Shell integration and local previews.
This capability does not itself request administrator privileges.

`unvirtualizedResources` is used only for the opt-in "Open Atlas with Win+E"
setting (off by default). The manifest excludes exactly two per-user keys from
registry virtualization — `HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}`
(the Win+E target) and `HKCU\Software\Atlas\Integration` (the backup of the
original value). Turning the setting off restores the original value and deletes
the backup. No other registry location is unvirtualized.

The ordinary local file-manager functions can be exercised without an account.
Cloud connections are optional and require the tester's account at the selected
provider; no Atlas account is needed for local browsing. If any submitted feature
is license-gated, identify that feature and provide working test access in the
private certification notes for that exact submission.

Encrypted private folders are local. Mounting them as a Windows filesystem uses
the optional third-party WinFsp 2.1 driver. Atlas does not bundle the driver in
its MSIX. When it is missing, an explicit user action downloads the official MSI
from https://github.com/winfsp/winfsp/releases/download/v2.1/winfsp-2.1.25156.msi,
verifies SHA-256 073a70e00f77423e34bed98b86e600def93393ba5822204fac57a29324db9f7a,
and launches Windows Installer with /passive /norestart. The UI links the WinFsp
license before installation. The user approves any administrator prompt. Atlas
retries mounting after success; cancellation preserves encrypted files, and
restart-required installer results are explained. Ordinary local file management
does not require this driver.

PLEASE REVIEW THIS OPTIONAL DRIVER FLOW SPECIFICALLY: policies 10.1.5 and 10.2.4
restrict external driver acquisition/dependencies. We are disclosing the exact
flow for assessment, not asserting an exception or WHCP approval that has not
been verified. If this flow is not accepted, a Store-specific driver-free/manual
integration build will be needed.

Corresponding source for Windows package 0.1.1.0:
https://atlasfm.modhyt.org/source-code
https://pub-89dd9e8d0b06444ca2f59e0dc245dbc9.r2.dev/source/atlas-0.1.1-source.zip
The source includes build instructions and locked dependency sources. The JSON
receipt beside the ZIP records hashes. Icons8 artwork remains separately licensed;
the source helper recovers exact PNG inputs from the recipient's binary/MSIX.

Settings → About provides an offline third-party notices document, attribution
to Icons8 and WinFsp, and links to privacy and terms. The privacy policy explains
local processing, optional remote-file transfers, startup update checks and
optional tool/theme downloads. Website analytics are separate from the desktop
application.

Confirm in the actual submission:

- The edition, purchase price and entitlement match the Store listing.
- External purchase declarations reflect actual in-app purchase links/flows.
- All required provider binaries and notices are present in a clean installation.
- Corresponding-source access complies with the applicable copyleft licenses.
- Installation, update, uninstallation and file-operation safety have been tested,
  including operation without internet and without WinFsp.

Additional bundled-tool evidence: RCLONE-THIRD-PARTY-NOTICES.txt covers the recorded Go modules in rclone v1.74.4; matching tool-source parts and checksums are linked at https://atlasfm.modhyt.org/source-code.

Local release validation and remaining UAC/runtime checks are recorded in docs/release-0.1.1-2026-10-31.md. These are not claims of a WACK pass or Microsoft approval.


Runtime dependency correction prepared on 2026-11-12:

the source manifest now declares Microsoft.VCLibs.140.00.UWPDesktop,

MinVersion 14.0.24217.0, published by Microsoft. The framework is installed and

updated through Microsoft Store. Atlas and its three Rust companion executables

in the inspected 0.1.5.0 MSIX import VCRUNTIME140.dll; that existing MSIX omitted

the dependency. The manifest change applies to future packages, not to the

already uploaded 0.1.5.0 artifact. Do not claim the submitted package is fixed

until a new package has been built, inspected and tested on clean Windows.

For a local sideload test, supply the matching x64 Desktop VCLibs framework to

Add-AppxPackage with -DependencyPath. The generic UWP VCLibs package and a

machine-wide vc_redist installation are not substitutes for this declaration.

Reference: https://learn.microsoft.com/en-us/troubleshoot/developer/visualstudio/cpp/libraries/c-runtime-packages-desktop-bridge
