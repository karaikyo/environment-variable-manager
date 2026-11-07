# Run and manually update Environment Manager

Extract the x64 application package into a writable local folder and run
`environment-variable-manager.exe`. Keep its application files together.
Sign-in startup is enabled by default after healthy first initialization; change it
in the main window when desired. Closing the main window keeps the application
running in the system tray. Use **Exit** to stop it.

Windows is authoritative for current variables. Settings, baselines, observed
history and pending-operation information live in `data/environment.sqlite`
beside the executable. The database and its backups are unencrypted.

To update:

1. Use **Exit** in the application or tray menu and wait for the process to stop.
   Closing the main window keeps monitoring in the tray. If drafts exist, choose
   **Exit and discard unsaved edits** or **Cancel exit**; unsaved drafts are
   session-only. Exit preserves saved changes and pending recovery information.
2. Keep the complete existing `data/` directory, including retained backups.
   Replace only application files from the new clean package. A replacement
   package must never include somebody else's `data/`, history or settings.
3. Run the new executable. Compatible data opens in place. A format upgrade first
   creates a separate verified backup in `data/`, then commits its changes and
   schema version together. A failure preserves the database and reports recording
   interruption inline; keep the files and use Refresh to retry after resolving
   the problem. Browsing remains available wherever Windows values are readable.

Migration backups use `before-schema-1-to-2.sqlite` and numbered suffixes for
retries. Earlier `before-operations.sqlite` and `before-settings.sqlite` backups
also remain untouched. Failed backup attempts may leave an incomplete destination;
retry reserves a different name and never assumes an existing file is verified.
Backups are separate retained copies: **clearing live completed history does not
clear a migration backup**, the baseline or pending-operation recovery information.
Anyone who can read a retained backup may still read its older history.

An older executable that cannot read a newer schema refuses unsafe use. It never
downgrades or replaces the database. Keep the original database and backup, and
use an application version supporting that schema. There is no automatic data
rollback or variable Restore action.

To move the application on the same computer and account, exit it and move the
whole folder including `data/`. Manually run the executable at its new location.
If its saved startup preference is enabled, its exact previously owned Run entry
is still present, and Windows has not blocked startup, the app updates that entry
to the new quoted path. Disabled, missing, blocked or foreign registrations and
unrelated startup entries are left alone. Windows blocks affect effective status
without erasing the saved enabled preference.

Updates work offline. There are no automatic update checks/downloads, accounts,
profiles or cross-machine history migration. Do not include personal `data/` when
sharing an application package.
