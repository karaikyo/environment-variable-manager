# Manual update-check boundaries

The public `update_check::UpdateChecker` is session-only and starts no work in its
constructor. `start()` accepts one explicit request, clears any previous result,
and returns false if already running. `poll()` publishes a finished result without
blocking; `status()` exposes Idle, Checking, UpToDate, Available or Failed.
Available releases expose their version and a constructed, fixed-project HTTPS
URL. No release metadata is executed or written to the application database.

Call `cancel()` during an accepted Exit, then continue polling until `is_running()`
is false before destroying the window. `Drop` requests cancellation without
blocking the UI; the worker never holds a window handle. A completed request does
not time out merely because the UI polls it later.

The external-response fixture boundary is `ReleaseTransport::fetch`, accepting a
URL and cancellation/deadline control and returning status, bytes and optional
Link header. Tests use prepared responses. Production uses `WindowsTransport`.
Fixtures must honor the control, just as the production adapter does.

Bounds: 15 seconds for a complete check; 5 pages of at most 100 releases each;
1 MiB per page and 2 MiB total; 8 KiB pagination text; 64 KiB native response
headers. Native operations are asynchronous, polled for cancellation at most
every 25 ms, with 3-second phase timeouts as fallback. Closing an async request
cancels its pending work. Cleanup drains at most another 100 ms; Windows retains
the callback/read-buffer context until the final handle-closing notification.
An outstanding teardown gate prevents retry accumulation. The WinHTTP library
is loaded only from System32, on the first explicit check, and remains loaded for
process lifetime because final callbacks return through its code. Measure its
post-check memory contribution in product acceptance.

Only this project's API path (including its exact numeric repository-ID alias)
is accepted. Continuations must advance by one page and are reconstructed as
canonical requests. TLS verification is enabled; redirects, authentication,
automatic credential forwarding and cookies are disabled. HTTP errors are not
parsed as release lists. Missing/incomplete/oversized results cannot claim Up to
date. Release eligibility includes published previews with the expected uploaded
Windows x64 ZIP. JSON is parsed by bundled SQLite in an ephemeral in-memory
connection; no new package dependency or persistent database is used.

For the separate real-connection acceptance check, build the `check_updates`
example with the installed offline toolchain and explicitly run it with `--check`.
It downloads only release metadata and prints the check result; it never opens a
browser, modifies Windows variables, registers startup or downloads a package.
