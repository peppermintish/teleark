# Desktop Preferences Format

Status: implemented local format, version 1

TeleArk stores desktop preferences in the Library SQLite `settings` table. The
frontend exchanges one typed `DesktopPreferences` value with the runtime and
never reads these rows directly. A save validates the entire value and writes
all keys in one storage transaction.

## Namespace and scalar encoding

Version 1 uses the `preferences.v1.` prefix. Boolean values are exactly `true`
or `false`; integers are canonical base-10 strings; appearance is `system`,
`light`, or `dark`. `managed_files_root` is a UTF-8 path, with the empty string
selecting TeleArk's platform application-data directory. The runtime derives
`Downloads`, `Cache`, and `Logs` children from this root and creates them together.

| Key suffix | Allowed value | Default |
| --- | --- | --- |
| `managed_files_root` | UTF-8 path or empty | empty |
| `download_directory` | retired UTF-8 path or empty | empty |
| `ask_download_destination` | retired boolean | `false` |
| `reveal_completed_downloads` | boolean | `false` |
| `upload_part_size_mib` | `1024` or `1900` | `1900` |
| `upload_encrypt_content` | boolean | `true` |
| `upload_hide_file_name` | boolean | `true` |
| `upload_encrypt_metadata` | boolean | `true` |
| `lock_vault_when_hidden` | boolean | `true` |
| `index_batch_size` | `200`, `500`, or `1000` | `1000` |
| `notify_download_completed` | boolean | `true` |
| `notify_download_failed` | boolean | `true` |
| `appearance` | `system`, `light`, or `dark` | `system` |

## Compatibility and failure behavior

Missing keys take their documented defaults, which lets older databases adopt
new version-1 settings. When `managed_files_root` is absent, a non-empty legacy
`download_directory` becomes the managed root; if its final component is
`Downloads` (case-insensitive), its parent becomes the root so the existing
download path is preserved. A subsequent save writes the new key and clears the
retired destination values. The retired
`ask_download_destination` value is ignored because downloads are now always
automatic. Unknown suffixes in the version-1 namespace are ignored for forward
compatibility. A malformed active known key makes the runtime report a
structured persistence error; the desktop falls back to defaults and shows
that settings could not be loaded or saved. A future incompatible encoding
must use a new namespace and migration rather than changing version 1 in place.

The managed root is not a permission grant. The runtime creates only its
`Downloads`, `Cache`, and `Logs` children, validates requested filenames, refuses path
traversal, and chooses a unique `name (n).extension` download destination
instead of replacing an existing file. SQLite and the Telegram session stay in
the platform application-data directory so changing this preference cannot
move files that are open by live workers.
