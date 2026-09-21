## TeleArk canonical English resource catalog.

common-search = Search
common-filter = Filter
common-view = View
common-upload = Upload
common-new-collection = New Collection
common-cancel = Cancel
common-save = Save
common-pause = Pause
common-resume = Resume
common-retry = Retry
channel-list-resize-hint = Drag the right divider to resize the channel list.
common-delete = Delete
common-open-file = Open File
common-open-folder = Open Folder
common-more = More
common-status = Status
common-size = Size
common-type = Type
common-source = Source
common-modified = Modified
common-encrypted = Encrypted
common-parts = Parts
common-details = Details
common-all = All
common-connected = Connected
common-account-count =
    { $count ->
        [one] { $count } account
       *[other] { $count } accounts
    }

menu-application-services = Services
menu-application-quit = Quit TeleArk
menu-view-title = View
menu-view-toggle-fullscreen = Toggle Full Screen
menu-window-title = Window
menu-window-minimize = Minimize
menu-window-zoom = Zoom

library-title = Library
library-all-files = All Files
library-recent = Recently Added
library-videos = Videos
library-documents = Documents
library-archives = Archives
library-audio = Audio
library-images = Images
library-disk-images = Disk Images
library-other = Other
library-channels = Channels
library-collections = Collections
library-storage-channel = TeleArk
library-transfers = Transfers
library-uploads = Uploads
library-downloads = Downloads
library-waiting = Waiting
library-completed = Completed
library-failed = Failed
library-search-placeholder = Search filenames, captions, tags, or channels
library-item-count =
    { $count ->
        [one] { $count } item
       *[other] { $count } items
    }
library-column-name = Name
library-local-storage = Local Storage
library-telegram-storage = Telegram Storage
library-storage-used = { $used } of { $total } used
library-empty-title = No files yet
library-empty-description = Index a channel or upload a file to start your library.
library-empty-description-local = Import files from this computer to start your persistent local library.
library-collection-preview-title = Collection preview
library-collection-preview-description = This preview collection is not connected to your saved library yet. Choose a library category to browse real files.
library-loading-title = Loading your library
library-loading-description = TeleArk is reading the local library index.
library-error-title = The library could not be loaded
library-result-count-dynamic =
    { $count ->
        [one] { $count } result
       *[other] { $count } results
    }
library-total-files-dynamic =
    { $count ->
        [one] { $count } file
       *[other] { $count } files
    }
library-local-index-size = { $size } indexed locally
library-first-page-note = Showing the first page
library-load-more = Load More
library-loading-more = Loading…
library-load-more-failed = Couldn’t load more files. Try again.
library-source-local = This computer
library-source-telegram-chat = Telegram chat { $chat_id }
library-file-picker-prompt = Import
library-choosing-files = Choosing files…
library-importing-files = Importing files…
library-import-success =
    { $count ->
        [one] Imported { $count } file.
       *[other] Imported { $count } files.
    }
library-import-partial = Imported { $imported } files; { $failed } could not be imported.
library-import-failed =
    { $count ->
        [one] { $count } file could not be imported.
       *[other] { $count } files could not be imported.
    }
library-import-empty = No files were selected.
library-picker-failed = The system file picker could not be opened.

action-import-files = Import Files

error-library-invalid-request = This library request is not valid.
error-library-not-found = The requested library item no longer exists.
error-library-conflict = This file is already present or conflicts with an existing item.
error-library-persistence = TeleArk could not read or save the local library.
error-library-source-missing = A selected source file can no longer be found.
error-library-source-changed = A selected source file changed while it was being imported.
error-library-permission-denied = TeleArk does not have permission to access this location.
error-library-capacity = The library cannot accept more data for this operation.
error-library-authorization = Authorization is required to complete this operation.
error-library-network = The network connection was interrupted.
error-library-cancelled = The operation was cancelled.
error-library-unknown = The library operation failed unexpectedly.

file-detail-empty-title = Select a file from the library
file-detail-empty-description = Import a file, then select it to view its saved details.
file-detail-modified-at = Modified At
file-detail-verification-unavailable = This catalog does not contain verified part or content-hash evidence.

upload-dialog-title = Upload to TeleArk
upload-target-account = Target Account
upload-target-channel = Target Channel
upload-storage-method = Storage Method
upload-automatic-multipart = Automatic multipart
upload-automatic-description = Split the file automatically into compatible { $size } parts.
upload-part-size = Part Size
upload-compatibility-mode = 1900 MiB compatibility mode
upload-part-size-mib = { $size } MiB
upload-custom-part-size = Custom part size
upload-security = Security
upload-client-encryption = Client-side encryption
upload-encryption-profile = Encryption Profile
upload-hide-filename = Hide original filename
upload-encrypt-metadata = Encrypt metadata such as name, size, and type
upload-estimate = Estimate
upload-part-count = Parts: { $count }
upload-total-size = Total size: { $size }
upload-telegram-messages = Telegram messages: { $count }
upload-estimated-time = Estimated time: { $time }
upload-change-file = Change Selection
upload-add-to-queue = Add to Upload Queue
upload-select-file = Select a File

transfer-title = Transfers
transfer-all-tasks = All Tasks
transfer-uploads = Uploads
transfer-downloads = Downloads
transfer-waiting = Waiting
transfer-completed = Completed
transfer-failed = Failed
transfer-start-all = Start All
transfer-pause-all = Pause All
transfer-retry-failed = Retry Failed
transfer-clear-completed = Clear Completed
transfer-new-queue = New Queue
transfer-settings = Transfer Settings
transfer-downloading-count = Downloading: { $count }
transfer-waiting-count = Waiting: { $count }
transfer-completed-count = Completed: { $count }
transfer-failed-count = Failed: { $count }
transfer-total-speed = Total speed: { $speed }
transfer-today-transferred = Today: { $size }
transfer-state-queued = Queued
transfer-state-uploading = Uploading
transfer-state-downloading = Downloading
transfer-state-paused = Paused
transfer-state-waiting-retry = Waiting to retry
transfer-state-verifying = Verifying
transfer-state-completed = Completed
transfer-state-failed = Failed
transfer-state-cancelled = Cancelled
transfer-eta = ETA: { $time }
transfer-remaining-time = Remaining: { $time }
transfer-parts-completed =
    { $completed ->
        [one] { $completed } of { $total } part complete
       *[other] { $completed } of { $total } parts complete
    }
transfer-queue-stats = { $running } running, { $waiting } waiting

file-detail-title = File Details
file-detail-overview = Overview
file-detail-parts = Parts
file-detail-details = Details
file-detail-activity = Activity
file-detail-uploaded-verified = Uploaded · Verified
file-detail-file-name = File Name
file-detail-size = Size
file-detail-type = Type
file-detail-source = Source
file-detail-uploaded-at = Uploaded At
file-detail-part-count = { $count } parts
file-detail-encryption-status = Encryption Status
file-detail-encryption-profile = Encryption Profile
file-detail-file-hash = File Hash
file-detail-part-number = Part
file-detail-telegram-message-id = Telegram Message ID
file-detail-open-location = Open File Location
file-detail-more-actions = More Actions

vault-title = Key Vault
vault-key-management = Key Management
vault-personal-master-key = Personal Master Key
vault-unlocked = Unlocked
vault-locked = Locked
vault-created-at = Created { $date }
vault-kdf = Key Derivation
vault-status = Status
vault-change-password = Change Password
vault-lock-vault = Lock Vault
vault-recovery-backup = Recovery and Backup
vault-recovery-key = Recovery Key
vault-backed-up = Backed Up
vault-not-backed-up = Not Backed Up
vault-show = Show
vault-export = Export
vault-warning-key-loss = Important: If you lose your password and recovery key, encrypted files stored in Telegram cannot be recovered.

index-title = Channel Index
index-status = Index Status
index-messages-scanned = Messages scanned: { $count }
index-files-indexed = Files indexed: { $count }
index-indexed-size = Indexed size: { $size }
index-latest-sync = Latest sync: { $date }
index-new-files = New files: { $count }
index-current-progress = Progress: { $percent }
index-current-date = Current date: { $date }
index-scan-speed = Scan speed: { $speed }
index-eta = Estimated time remaining: { $time }
index-start = Start Indexing
index-pause = Pause Indexing
index-resume = Resume Indexing
index-cancel = Cancel Indexing
index-coverage = Index Coverage
index-coverage-complete = Complete
index-coverage-partial = Partial
index-coverage-not-scanned = Not Scanned
index-last-30-days = Last 30 Days
index-last-6-months = Last 6 Months
index-last-year = Last Year
index-all-history = All History
index-custom-range = Custom Date Range
index-content-types = Content Types
index-files = Files
index-videos = Videos
index-images = Images
index-audio = Audio
index-plain-text = Plain Text Messages
index-skip-small-media = Skip media smaller than { $size }

settings-title = Settings
settings-general = General
settings-accounts = Accounts
settings-storage = Storage
settings-downloads = Downloads
settings-uploads = Uploads
settings-key-vault = Encryption keys
settings-index = Index
settings-notifications = Notifications
settings-appearance = Appearance
settings-advanced = Advanced
settings-language-title = Language
settings-language-description = Choose the language used by TeleArk.
settings-language-system-default = System Default
settings-language-english = English
settings-language-chinese = Simplified Chinese
settings-language-japanese = Japanese
settings-theme = Theme
settings-theme-system = Follow System
settings-theme-light = Light
settings-theme-dark = Dark
settings-concurrency = Concurrent Transfers
settings-bandwidth = Bandwidth Limit

error-transfer-network = The network connection was interrupted. Check your connection and retry.
error-transfer-flood-wait = Telegram asked TeleArk to wait { $seconds } seconds before retrying.
error-transfer-authorization = This Telegram account is not authorized for the requested transfer.
error-transfer-source-missing = The source file can no longer be found.
error-transfer-source-changed = The source file changed after the transfer started. Start a new upload.
error-transfer-disk-full = There is not enough free space at the destination.
error-transfer-permission-denied = TeleArk does not have permission to access this location.
error-transfer-remote-missing = A required Telegram object is missing.
error-transfer-hash-mismatch = Integrity verification failed because the file hash did not match.
error-transfer-authentication-failed = Encrypted content authentication failed.
error-transfer-manifest-corrupted = The package manifest is damaged or invalid.
error-transfer-unsupported-manifest = Manifest version { $version } is not supported by this version of TeleArk.
error-transfer-key-unavailable = The encryption key required for this file is unavailable.
error-transfer-wrong-password = The vault password is incorrect.
error-transfer-database = TeleArk could not save transfer state. Retrying may resolve the problem.
error-transfer-cancelled = The transfer was cancelled.
error-transfer-unknown = The transfer failed because of an unknown error.

## Semantic IDs consumed by the GPUI reference screens.

search-placeholder = Search filenames, captions, or channels
connection-connected = Connected
accounts-count = 3 accounts
account-standard = Standard account
channel-private = Private channel
status-healthy = Healthy
storage-local = Local storage
storage-telegram = Telegram storage
overall-title = Overall
overall-download-speed = Download
overall-upload-speed = Upload
overall-free-space = Free disk
overall-app-usage = App usage

action-cancel = { common-cancel }
action-upload = { common-upload }
action-download = Download
action-filter = { common-filter }
action-start-all = Start All
action-pause-all = Pause All
action-resume-all = Resume All
action-retry-failed = Retry Failed
action-clear-completed = Clear Completed
action-delete-task = Delete Task
action-confirm-delete-task = Confirm Delete (Keep File)
action-new-queue = New Queue
action-pause = { common-pause }
action-resume = { common-resume }
action-retry = { common-retry }
action-open-file = { common-open-file }
action-open-location = Open Location
file-detail-local-path = Local path
collection-new = { common-new-collection }

nav-library = Library
nav-all-files = All Files
nav-recent = Recent
nav-videos = Videos
nav-documents = Documents
nav-archives = Archives
nav-channels = Channels
nav-collections = Collections
nav-transfers = Transfers
nav-all-transfers = All Transfers
nav-completed = Completed
nav-failed = Failed
nav-storage = Storage
nav-key-vault = Encryption keys
nav-settings = Settings

filter-all-channels = All Channels
filter-video = Video
filter-large-files = Larger than 1 GB
filter-all-statuses = All Statuses

table-name = Name
table-size = Size
table-type = Type
table-source = Source
table-modified = Modified
table-status = Status
table-encrypted = Encrypted
table-parts = Parts
table-progress = Progress
table-speed = Speed
table-destination = Destination

library-result-count = 128,974 results
library-total-files = 128,974 files

file-type-video = Video
file-type-disk-image = Disk Image
file-type-document = Document
file-type-archive = Archive
file-type-image = Image
file-type-audio = Audio
file-type-other = Other
file-state-remote = Remote
file-state-downloaded = Downloaded
file-state-uploaded = Uploaded
file-state-verified = Verified
file-state-encrypted = Encrypted
file-state-local = Local
file-state-uploading = Uploading
file-state-verifying = Verifying
file-state-verification-failed = Verification failed
file-state-remote-missing = Remote file missing
file-state-locked = Locked

transfer-state-waiting = Waiting
transfer-summary-downloading = Downloading
transfer-summary-task-count = { $count } tasks
transfer-summary-completed-note = Finished downloads
transfer-summary-live-runtime = Live Telegram runtime
transfer-summary-completed-data = Completed data
transfer-value-unavailable = —
transfer-summary-tasks = 8 tasks
transfer-summary-waiting = Waiting
transfer-summary-ready = Ready to start
transfer-summary-completed = Completed
transfer-summary-today = Today +128
transfer-summary-failed = Failed
transfer-summary-retry = Needs retry
transfer-summary-total-speed = Total Speed
transfer-summary-today-data = Transferred Today
transfer-summary-month-change = 28% more than yesterday
transfer-footer-total = 166 tasks
transfer-footer-downloading = Downloading 8 (78.27 GB)
transfer-footer-waiting = Waiting 156 (210.43 GB)
transfer-footer-unlimited = Limit: Unlimited
transfer-footer-total-live = { $count } tasks
transfer-footer-downloading-live = { $count } downloading
transfer-footer-waiting-live = { $count } waiting
transfer-empty = No transfers yet. Choose a Telegram source and download a file to get started.
transfer-batch-name = { $source } · { $count } files
transfer-tab-log = Log

connection-title = Connections
connection-address = IP Address:Port
connection-client = Client
connection-latency = Latency

detail-source-channel = Source Channel
detail-message-id = Message ID
detail-local-path = Local Path
detail-speed = Transfer speed
detail-downloaded = Downloaded
detail-concurrency-limits = Concurrency limits
detail-retries = Retries
detail-created = Created
detail-started = Started
detail-time-remaining = Time Remaining
detail-tab-details = Details
detail-tab-file-list = Files
detail-verification = File Verification

log-connected = Connected to peer
log-block-downloaded = Block downloaded
log-file-list = Retrieved file list
log-verified = Download complete; verification passed
log-connection-timeout = Connection timed out; retrying
log-saved = Download complete; file saved

upload-source-unchanged = Source unchanged
upload-source-checked = Checked just now
upload-automatic-multipart-description = Split this file into Telegram-compatible application parts.
upload-conservative-mode = 1024 MiB conservative mode
upload-estimate-title = Estimated Information
upload-estimate-parts = Part Count
upload-estimate-part-size = Part Size
upload-estimate-total-size = Total Size
upload-estimate-messages = Telegram Messages
upload-estimate-time = Estimated Time
upload-client-encryption-description = Encrypt content locally before upload.
upload-hide-filename-description = Store a protected name instead of the original filename.
upload-encrypt-metadata-description = Protect filename, size, and type metadata.

file-detail-hash = File Hash
file-detail-part-size = Part Size
file-detail-encryption = Encryption
file-detail-frame-size = Crypto Frame Size
file-detail-manifest = Manifest
file-detail-package-id = Package ID
file-detail-tab-parts = Parts
file-detail-tab-details = Details
file-detail-tab-activity = Activity
file-detail-all-parts-verified = All parts verified
file-detail-manifest-synced = Manifest synchronized
file-detail-part-index = Part
file-detail-remote-id = Telegram Message ID
file-detail-upload-time = Upload Time

index-channel-title = Channel Index Detail
index-options = Index Options
index-status-synced = Synchronized
index-channel-description = Local metadata index with incremental and historical coverage.
index-since-last-sync = Since last sync
index-history-scan = Historical scan
index-batch-size = Batch size: 1,000
index-estimated-time = Estimated Time
index-until-complete = Until complete
index-coverage-title = Historical Index Coverage
index-coverage-description = Stored ranges show complete, partial, and unscanned history.
index-range-complete = Complete Range
index-range-active = Active Range
index-range-unscanned = Unscanned Range
index-current-job = Current Index Job
index-state-paused = Paused
index-state-indexing = Indexing
index-range-progress = Range Progress
index-job-range = Date Range
index-job-checkpoint = Checkpoint
index-job-checkpoint-value = Message { $id }
index-job-files-found = Files Found
index-job-errors = Errors
index-job-updated = Last Updated
index-coverage-missing = Not Scanned

settings-saved = Saved
settings-indexing = Indexing
settings-language-runtime-note = Language changes apply immediately to every open TeleArk screen.
settings-language-persistence-ready = Language preference is stored on this Mac.
settings-language-persistence-saving = Saving language preference…
settings-language-persistence-saved = Language preference saved
settings-language-persistence-failed = Language preference could not be saved
settings-theme-system-description = Use the current operating-system appearance.
settings-theme-light-description = Always use the light appearance.
settings-theme-dark-description = Always use the dark appearance.
settings-behavior-title = Application Behavior
settings-start-at-login = Start TeleArk at Login
settings-start-at-login-description = Open TeleArk automatically when you sign in.
settings-restore-window = Restore Last Window
settings-restore-window-description = Reopen the screen used in the previous session.
settings-show-menu-bar = Show Menu Bar Status
settings-show-menu-bar-description = Keep transfer status available from the system menu bar.

vault-master-key = Personal Master Key
vault-status-locked = Locked
vault-status-unlocked = Unlocked
vault-created = Created
vault-cipher = Content Cipher
vault-unlock = Unlock Vault
vault-lock = Lock Vault
vault-recovery-title = Recovery Key
vault-recovery-backed-up = Backed Up
vault-show-recovery = Show Recovery Key
vault-hide-recovery = Hide Recovery Key
vault-export-recovery = Export Recovery Key
vault-profile-title = Default Encryption Profile
vault-option-hidden-filenames = Hidden Filenames
vault-option-hidden-filenames-description = Do not expose original names in Telegram.
vault-option-encrypted-metadata = Encrypted Metadata
vault-option-encrypted-metadata-description = Protect names, sizes, and file types.
vault-option-compatible-parts = Compatible Parts
vault-option-compatible-parts-description = Use 1900 MiB application parts.
vault-option-keychain = OS Credential Store
vault-option-keychain-description = Store the wrapping secret in the operating-system keychain.
vault-key-loss-warning = If both the system keychain entry and recovery bundle are lost, encrypted files cannot be recovered.
vault-unlock-to-view = Unlock vault to view

prototype-demo-badge = Preview build
common-not-applicable = Not applicable
file-detail-not-encrypted = Not encrypted
file-detail-parts-pending = Part verification pending
file-detail-manifest-pending = Manifest pending
action-start = Start
table-eta = ETA
detail-transferred = Transferred
detail-verification-passed = BLAKE3 verified
detail-verification-pending = Verification pending
detail-verification-failed = Verification failed
settings-session-only = Session only
settings-preview-controls = Other settings are Preview
action-view-options = View options
action-back = Back
action-more = More actions
telegram-library-title = Telegram Sources
telegram-status-working = Working…
telegram-status-failed = Needs attention
telegram-status-connected = Connected
telegram-status-not-connected = Not connected
telegram-header-login-action = Sign in
window-exit-fullscreen-action = Exit full screen
telegram-login-title = Sign in to Telegram
telegram-login-description = Choose either method below. Telegram asks for your two-step verification password only after the login code when it is enabled.
telegram-phone-login-title = Sign in with phone number
telegram-phone-login-description = Enter your phone number to receive a login code. TeleArk uses the API credentials saved in Settings.
telegram-api-id-label = API ID
telegram-api-hash-label = API Hash
telegram-phone-label = Phone number
telegram-api-id-placeholder = Numeric API ID
telegram-api-hash-placeholder = API hash
telegram-credentials-prompt-title = Set up Telegram API access
telegram-credentials-prompt-description = Enter your own API ID and API Hash to enable Telegram sign-in, channel browsing, and downloads. You can skip for now and add them later in Settings.
telegram-credentials-save-action = Save credentials
telegram-api-id-skip-action = Skip for now
telegram-api-id-required-title = Telegram sign-in is disabled
telegram-api-id-required-description = Save your own API ID and API Hash in Settings to enable phone and QR sign-in. TeleArk does not use shared Telegram Desktop credentials.
telegram-configure-api-action = Configure Telegram API
telegram-credentials-official-panel-note = Create and manage your application credentials in Telegram's official API development panel. If you lose the API Hash, return to the panel to manage or recreate your application credentials.
telegram-phone-placeholder = International phone number
telegram-code-placeholder = Login code
telegram-password-placeholder = Two-step verification password
telegram-connect-action = Connect and send code
telegram-connect-qr-action = Sign in with QR code
telegram-qr-title = Scan with Telegram
telegram-qr-description = In the Telegram mobile app, open Settings › Devices › Link Desktop Device, then scan this code.
telegram-qr-placeholder = Enter the API ID and API Hash, then generate a secure QR code.
telegram-qr-credentials-placeholder = Credentials required
telegram-qr-refresh-note = The code refreshes automatically when it expires. Keep this window private.
telegram-qr-refresh-action = Refresh code now
telegram-qr-use-phone-action = Use phone number instead
telegram-code-title = Enter the login code
telegram-code-description = Telegram sent a code to your account. Enter it below to continue.
telegram-code-action = Verify code
telegram-password-title = Two-step verification
telegram-password-description = Enter your Telegram two-step verification password.
telegram-password-hint = Password hint: { $hint }
telegram-password-action = Unlock account
settings-telegram-credentials-title = Telegram API credentials
settings-telegram-credentials-description = Custom API credentials are optional and off by default. TeleArk uses its bundled credentials until you save your own.
settings-telegram-credentials-save-action = Save credentials
settings-telegram-credentials-clear-action = Remove saved credentials
settings-telegram-api-id-missing = Not configured
settings-telegram-api-id-configured = Configured
settings-telegram-api-id-distribution = Built-in release credentials
settings-telegram-api-id-saving = Saving…
settings-telegram-api-id-saved = Saved
settings-telegram-credentials-removed = Removed
settings-telegram-credentials-removed-using-distribution = Personal credentials removed; using built-in release credentials
settings-telegram-api-id-invalid = Enter a positive numeric API ID and a 32-character hexadecimal API Hash.
settings-telegram-api-id-failed = The credentials could not be saved. Check local storage and try again.
settings-telegram-credentials-storage-note = Your API ID and API Hash are stored in TeleArk's local SQLite Library database. Protect your macOS account and backups. Personal credentials override any credentials supplied by the TeleArk distributor.
settings-telegram-credentials-distribution-note = This build is using API credentials registered by its TeleArk distributor, so sign-in works without additional setup. You can save your own application credentials above to override them. Shared Telegram Desktop credentials are never used.
settings-telegram-api-panel-action = Open Telegram API development panel
settings-preferences-ready = Settings are stored locally
settings-preferences-saving = Saving settings…
settings-preferences-saved = Settings saved
settings-preferences-failed = Settings could not be saved
settings-storage-library-title = Local library database
settings-storage-library-description = Review the SQLite database used for accounts, indexes, transfers, checkpoints, and preferences.
settings-path-unavailable = Path unavailable
settings-storage-reveal-action = Show database in Finder
settings-storage-sqlite-note = TeleArk owns this database. Close TeleArk before copying it for backup, and do not edit it with another application.
settings-download-title = Managed files and downloads
settings-download-description = TeleArk keeps downloads, cache, and diagnostic logs together under one location. Downloads start automatically without asking where to save each file.
settings-managed-root-label = TeleArk managed files location
settings-managed-downloads-label = Downloads
settings-managed-cache-label = Cache
settings-managed-root-action = Choose managed files location
settings-managed-root-default-action = Use default location
settings-managed-root-open-action = Open managed files folder
settings-managed-root-picker = Choose the folder that will contain TeleArk Downloads, Cache, and Logs
settings-download-directory-unset = Ask for a destination when each download starts
settings-download-directory-action = Choose download folder
settings-download-directory-clear-action = Clear default folder
settings-download-directory-picker = Choose the default download folder
settings-download-ask-each-time = Ask where to save every file
settings-download-ask-each-time-description = Show a save dialog for every new Telegram download. Choose a default folder before turning this off.
settings-download-reveal-completed = Show completed downloads in Finder
settings-download-reveal-completed-description = Reveal the downloaded file automatically after verification succeeds.
settings-upload-title = Upload defaults
settings-upload-description = Encrypted uploads use your private TeleArk channel.
settings-upload-vault-managed-title = Vault-managed encrypted uploads
settings-upload-vault-managed-description = TeleArk automatically encrypts content, names and metadata. Containers hold up to { $size } of plaintext; encryption and upload advance together in 512 KiB blocks.
upload-current-part-size = Current safe part limit
upload-current-part-size-description = TeleArk currently creates encrypted plaintext parts of at most { $size }.
settings-vault-title = Key Vault protection
settings-vault-description = Manage file encryption keys and recovery. Configure the application PIN in General settings.
settings-index-title = Telegram indexing
settings-index-description = Choose how many messages TeleArk scans in each channel indexing request.
settings-index-batch-option = { $count } messages
settings-index-batch-option-description = Messages per scan
settings-notification-title = Transfer notifications
settings-notification-description = Choose which download results appear as in-app notifications.
settings-notify-download-completed = Download completed
settings-notify-download-completed-description = Show a success notification after a file is verified.
settings-notify-download-failed = Download failed
settings-notify-download-failed-description = Show an error notification when a transfer cannot finish.
settings-appearance-title = Appearance
settings-appearance-description = Choose how TeleArk colors its windows and controls.
notification-download-completed = Download completed: { $name }
notification-download-failed = Download failed: { $name }
action-show-in-folder = Show in Finder
transfer-footer-selected = { $count } selected
telegram-channel-select-title = Choose a source
telegram-no-channel-selected = No source selected
telegram-index-description = Scan this source in bounded pages and save discovered files to the local searchable library.
telegram-index-next-action = Scan next 1,000 messages
telegram-index-complete = Scan complete. { $count } files are indexed in total.
telegram-index-page-complete = Page complete. { $count } files were saved. Continue to scan older messages.
telegram-files-title = Downloadable files ({ $count })
telegram-files-selected = { $count } selected
telegram-files-select-all = Select all results
telegram-files-clear-selection = Clear selection
telegram-files-refresh-action = Refresh files
telegram-files-loading = Loading files from this source…
telegram-files-fetching-progress = Fetching new messages… { $scanned } of { $target } examined
telegram-files-fetching-slow = Telegram is responding slowly. You can cancel and retry.
telegram-files-cancel-action = Cancel
telegram-files-retry-action = Retry
telegram-files-load-failed = New messages could not be loaded. Check the connection and retry.
telegram-files-empty = No downloadable documents were found in this page.
telegram-file-unnamed = Telegram document { $message_id }
telegram-file-metadata = { $size } · Message { $message_id }
telegram-file-download-action = Download
telegram-file-select-action = Select file
telegram-files-more-action = Load older files
telegram-batch-title = Batch download
telegram-batch-description = Filter the loaded page by sent time and one or more file types.
telegram-batch-period-label = Sent during
telegram-batch-period-any = Any time
telegram-batch-period-24h = Past 24 hours
telegram-batch-period-7d = Past 7 days
telegram-batch-period-30d = Past 30 days
telegram-batch-kind-label = File type
telegram-batch-kind-all = All files
telegram-batch-kind-video = Videos
telegram-batch-kind-document = Documents
telegram-batch-kind-archive = Archives
telegram-batch-kind-audio = Audio
telegram-batch-kind-image = Images
telegram-batch-kind-other = Other
telegram-batch-download-action = Download selected
telegram-batch-preparing = Preparing selected files…
telegram-batch-queued = Added { $count } files as one download group.
telegram-batch-no-matches = No files are selected.
telegram-batch-failed = The batch could not be prepared. Check the connection and try again.
telegram-message-detail-title = Message details
telegram-message-detail-empty = Select a file to view its message details.
telegram-message-file-name = File name
telegram-message-sent-at = Sent at
telegram-message-mime-type = Media type
telegram-message-caption = Caption
telegram-message-no-caption = No caption
telegram-download-queued = Download queued
telegram-download-running = Downloading
telegram-download-paused = Download paused
telegram-download-completed = Download complete
telegram-download-failed = Download failed
telegram-download-cancelled = Download cancelled
detail-verification-size-checked = Telegram size check passed
telegram-error-invalid-request = Check the API credentials, phone number, code, or password and try again.
telegram-error-authorization = Telegram authorization expired. Connect the account again.
telegram-error-network = Telegram could not be reached. Check the network and retry.
telegram-error-persistence = The local Telegram session could not be opened safely.
telegram-error-generic = Telegram could not complete this operation.
detail-finished = Finished
detail-trace-id = Trace ID
detail-queue-wait = Queue wait
detail-elapsed = Elapsed
detail-average-speed = Average speed
detail-failure-reason = Failure reason
detail-failure-part-context = Part { $part } · attempt { $attempt } · connection { $connection } · { $elapsed }
detail-failure-part-context-legacy = Part { $part } · attempt { $attempt } · waited { $elapsed }. This older log did not record the exact cause or connection.
detail-failure-retryable = This failure may be temporary. Check the conditions above and retry.
detail-failure-timeout-guidance = Retry the download. If requests keep timing out, check your network or proxy connection to Telegram.
detail-failure-user-action = This failure needs a settings, account, source, or filesystem change before retrying.
detail-failure-terminal = This task stopped and will not retry automatically.
detail-verification-not-reached = Verification has not been performed
detail-trace-timeline = Diagnostic timeline
trace-event-queued = Queued
trace-event-started = Started
trace-event-paused = Paused
trace-event-resumed = Resumed
trace-event-completed = Completed
trace-event-failed = Failed
trace-event-cancelled = Cancelled
native-download-error-invalid-request = The download request or destination was invalid.
native-download-error-not-found = The Telegram message or document no longer exists.
native-download-error-conflict = Another active task is already using this destination.
native-download-error-persistence = TeleArk could not safely read or write local transfer state.
native-download-error-source-missing = The Telegram source document is no longer available.
native-download-error-source-changed = The Telegram source changed while it was being downloaded.
native-download-error-permission-denied = macOS denied access to the configured download location.
native-download-error-capacity = The transfer queue or local resource limit was reached.
native-download-error-authorization = Telegram authorization expired or does not allow this download.
native-download-error-network = The network connection or Telegram request was interrupted.
native-part-error-timeout = Telegram did not answer the part request within 60 seconds.
native-part-error-network = The part request lost its Telegram connection.
native-part-error-server = Telegram reported a server error for this part.
native-part-error-rate-limited = Telegram asked this part request to wait.
native-part-error-authorization = Telegram rejected the account authorization for this part.
native-part-error-unexpected-response = Telegram returned incomplete or unexpected part data.
native-part-error-other = The part request stopped for another transfer error.
native-download-error-cancelled = The download was cancelled before completion.
native-download-error-unknown = The download failed for an unclassified internal reason.
settings-managed-logs-label = Diagnostic logs
settings-managed-logs-restart-note = Diagnostic logs will move to this location the next time TeleArk starts.
settings-diagnostics-title = Diagnostics and performance tracing
settings-diagnostics-description = TeleArk records structured operation timing, transfer lifecycle events, and safe failure categories in daily JSON logs.
settings-diagnostics-unavailable = Structured diagnostic logging is unavailable for this run
settings-diagnostics-dropped-events = { $count } diagnostic events dropped because the bounded writer was full
settings-diagnostics-open-action = Open diagnostic logs
settings-diagnostics-privacy-note = Logs may contain technical task, channel, and message IDs, byte counts, timings, and error categories. They never include API Hashes, login tokens, passwords, file contents, phone numbers, filenames, captions, or local file paths.
nav-no-channels = No channels found
nav-storage-channel = TeleArk
nav-storage-channel-telegram-files = Telegram Files
nav-storage-channel-teleark-files = TeleArk Files
storage-channel-title = TeleArk Storage
storage-channel-telegram-files = Raw Files
storage-channel-teleark-files = Files
storage-channel-tabs-description = Raw Telegram objects and reconstructed logical files
storage-channel-managed-title = TeleArk-managed files
storage-channel-managed-runtime-note = TeleArk recognizes package manifests and parts here. Automatic decryption and reconstruction will become available when the desktop Vault owner is connected; this alpha does not claim that locked packages are restored.
storage-channel-managed-empty = No authenticated files yet. Upload your first file or refresh to scan this channel.
storage-channel-managed-name-locked = Encrypted logical file
storage-channel-managed-detail-title = File details
storage-channel-managed-detail-empty = Select a managed file to inspect its package.
vault-password-placeholder = Enter a Vault password
vault-new-password-placeholder = Confirm the new password
vault-recovery-placeholder = Paste the complete recovery bundle
vault-status-not-configured = Not configured
vault-password-generation = Password wrap v{ $generation }
vault-operation-working = Working…
vault-operation-succeeded = Vault operation completed.
vault-create-password-label = Create password
vault-confirm-password-label = Confirm password
vault-create-action = Create Key Vault
vault-password-label = Password
vault-unlock-password-action = Unlock with password
vault-recovery-key-label = Recovery key
vault-unlock-recovery-action = Unlock with recovery key
vault-new-password-label = New password
vault-rotate-recovery-action = Replace recovery key
vault-recovery-save-now-title = Save this recovery key now
vault-recovery-save-now-description = This is the only time TeleArk displays this self-contained recovery bundle. Store it offline before hiding it. Replacing the current recovery key does not revoke older exported disaster-recovery bundles; protect or securely remove old copies.
vault-restore-title = Restore after local data loss
vault-restore-description = Enter a self-contained recovery bundle above and choose the new local password in both password fields.
vault-recovery-bundle-label = Recovery bundle
vault-recovery-export-default-name = TeleArk Recovery Bundle.txt
vault-restore-action = Restore Key Vault
vault-os-credential-title = OS Credential
vault-os-credential-development-note = This feature is under development and cannot be selected yet.
vault-error-invalid-request = Check the recovery bundle and try again.
vault-error-authorization = The recovery bundle could not be authenticated.
vault-error-source-missing = The selected source or remote package is unavailable.
vault-error-permission-denied = TeleArk does not have permission to access that location.
vault-error-network = Telegram could not complete the Vault operation. Try again.
vault-error-not-found = The requested Vault or package was not found.
vault-error-conflict = Encryption key settings changed. Retry the operation.
vault-error-capacity = The operation exceeded a supported size or storage limit.
vault-error-cancelled = The Vault operation was cancelled.
vault-error-persistence = Vault data could not be safely read, verified, or saved.
upload-file-picker-prompt = Choose Files
upload-no-file-selected = No file selected
upload-select-file-description = Select a source file. Its original path is never uploaded.
storage-channel-managed-vault-locked = Unlock this session to view original file names and browse encrypted files. Raw Files remains available while locked.
storage-channel-managed-runtime-ready = Authenticated manifests are shown as logical files. Downloads decrypt into a partial file, verify the whole file, then publish it in TeleArk Downloads.
storage-channel-manifest-authenticated = Authenticated
storage-channel-restore-ready = Ready to restore
storage-channel-download-restored-action = Download restored file
transfer-vault-storage-channel = Telegram / TeleArk (encrypted)
transfer-vault-encrypted-type = TeleArk encrypted package
transfer-vault-parts-progress = { $completed } / { $total } parts
detail-vault-lifecycle = Task durability
detail-vault-lifecycle-memory-only = Memory-only snapshot; pause, cancel, retry, and restart resume are not available yet
detail-vault-controls-unavailable = Running · controls unavailable
storage-channel-upload-action = Upload File
storage-channel-package-id = Package ID
storage-channel-logical-name = Original file name
storage-channel-manifest-state = Manifest
storage-channel-manifest-found = Manifest found
storage-channel-manifest-missing = Manifest missing
storage-channel-encoded-size = Telegram encoded size
storage-channel-restore-state = Restore state
storage-channel-restore-owner-unavailable = Waiting for the desktop Vault unlock and recovery owner
storage-channel-related-files = Related Telegram files
storage-channel-file-role = TeleArk file role
storage-channel-role-manifest = Self-describing manifest
storage-channel-role-part = Encrypted application part { $index }
storage-channel-why-file-exists = Why this file exists
storage-channel-manifest-explanation = This manifest describes the original logical file, its encrypted parts, integrity data, and the Telegram messages needed for recovery.
storage-channel-part-explanation = This opaque file contains one encrypted range of a larger logical file and is combined with the other package parts described by the manifest.
upload-target-storage-channel = TeleArk private channel
upload-storage-channel-security-note = Your file, name, and metadata are encrypted before upload to your private TeleArk channel.
detail-direction = Direction
detail-direction-upload = Upload
detail-direction-download = Download
detail-storage-format = Storage format
detail-storage-format-native = Native Telegram file (unchanged)
detail-storage-format-teleark = TeleArk encrypted multipart package
detail-content-protection = Content protection
detail-content-protection-none = None; Telegram stores the original file bytes
detail-content-protection-aes = AES-256-GCM authenticated encryption
detail-integrity-codec = Integrity and framing
detail-integrity-native = Telegram declared length; content hash is not yet available
detail-integrity-teleark = BLAKE3 plaintext/ciphertext digests; authenticated encryption frames
detail-manifest-codec = Manifest format
detail-manifest-codec-value = teleark-manifest-v1 (provisional)
transfer-preview-upload-note = Upload preview; the retained desktop encrypted-upload worker is not connected yet.
transfer-summary-uploading = Uploading
action-load-more = Load More
transfer-controller-title = Transfer activity
transfer-mode-live = Live
transfer-mode-replay = Replay
transfer-replay-previous = Previous
transfer-replay-next = Next
transfer-replay-position = Decision {$current} of {$total}
transfer-controller-phase = Controller phase
transfer-controller-parameters = C / W / F / P / E / Qe
transfer-controller-goodput = Actual goodput
transfer-controller-encryption-throughput = Encryption throughput
transfer-controller-disk-throughput = Disk throughput
transfer-controller-bdp = Estimated BDP
transfer-controller-rtt = RTT p95
transfer-controller-inflight = Inflight bytes
transfer-controller-inflight-value = {$current} current / {$target} target
transfer-controller-cpu = Encryption worker CPU utilization
transfer-controller-bottleneck = Detected bottleneck
transfer-controller-memory = Transfer memory
transfer-controller-memory-value = {$used} used / {$budget} budget
transfer-controller-part-map = Part map
transfer-controller-part-map-value = {$completed} completed · {$inflight} inflight · {$retry} retry · {$failed} failed · {$missing} missing
transfer-controller-connections-value = C={$connections} · W={$rpcs}
transfer-controller-queue-waits-value = Network waited {$network} for encryption · encryption waited {$encryption} for network · {$parts} parts/s
transfer-controller-buffers-value = Plaintext {$plaintext} · encrypted {$encrypted} · network {$network} · writer {$writer}
transfer-controller-decisions = Controller decisions
transfer-controller-no-decisions = Waiting for the first measured decision.
transfer-controller-lanes = DC and connection lanes
transfer-controller-lanes-unavailable = The active grammers transport does not expose trustworthy per-DC lane metrics yet.
transfer-lane-value = DC {$dc} · lane {$lane} · {$inflight} inflight · {$speed} · RTT p95 {$rtt} · {$status}
transfer-lane-active = active
transfer-lane-paused = paused
transfer-session-log = Permanent session log
transfer-phase-ramp = RAMP
transfer-phase-probe = PROBE
transfer-phase-stable = STABLE
transfer-phase-recover = RECOVER
transfer-bottleneck-unknown = Collecting evidence
transfer-bottleneck-encryption = CPU encryption limited
transfer-bottleneck-network = Telegram or network limited
transfer-bottleneck-disk = Disk limited
transfer-bottleneck-memory = Memory budget limited
transfer-controller-no-parameter = No parameter change
transfer-parameter-connections = Transfer connections (C)
transfer-parameter-rpcs = Inflight RPCs per connection (W)
transfer-parameter-files = Active files (F)
transfer-parameter-parts = Inflight parts per file (P)
transfer-parameter-encryption-workers = Encryption workers (E)
transfer-parameter-encrypted-queue = Encrypted queue depth (Qe)
transfer-decision-probe = PROBE
transfer-decision-keep = KEEP
transfer-decision-confirm = CONFIRM
transfer-decision-platform = PLATFORM
transfer-decision-rollback = ROLLBACK
transfer-decision-recover = RECOVER
transfer-decision-respect-soft-limit = RESPECT LIMIT
transfer-decision-override-soft-limit = OVERRIDE LIMIT
transfer-decision-ignore-soft-limit = IGNORE LIMIT
transfer-decision-pause-lane = PAUSE LANE
transfer-decision-resume-lane = RESUME LANE
transfer-reason-initial-ramp = Initial throughput ramp.
transfer-reason-bdp = Current inflight bytes are below the 1.75× BDP target.
transfer-reason-improved = The probe improved goodput by at least 3%.
transfer-reason-confirm = The probe improved goodput by 1–3%; another sample is required.
transfer-reason-platform = The probe gained less than 1%; the previous setting is the platform point.
transfer-reason-regressed = Goodput declined, so the controller restored the previous setting.
transfer-reason-encryption-starved = Network lanes spent too long waiting for encrypted parts.
transfer-reason-network-backpressure = The encrypted queue stayed near capacity, so encryption concurrency was reduced.
transfer-reason-small-files = The small-file queue needs more active file slots.
transfer-reason-large-file = A large-file pipeline needs more inflight parts.
transfer-reason-memory = Buffered and inflight bytes exceeded the transfer memory budget.
transfer-reason-disk = Disk throughput is limiting end-to-end goodput.
transfer-reason-part-retry = A transient part failure triggered retry and reduced per-file inflight concurrency.
transfer-reason-flood-wait = Telegram requires this lane to wait; other lanes remain eligible.
transfer-reason-flood-wait-expired = The required Telegram wait expired and the lane resumed.
transfer-reason-soft-limit = The measured probe conflicts with Telegram's conservative soft limit.
transfer-reason-all-platform = Every eligible parameter is at its measured platform or configured bound.
transfer-decision-throughput-value = {$elapsed} · {$before} → {$after} ({$change})
transfer-part-timeline = Recent part timeline
transfer-part-retention = Showing the latest { $shown } part events; { $omitted } older events omitted.
transfer-part-inflight = Inflight
transfer-part-completed = Completed
transfer-part-retry = Retry
transfer-part-failed = Failed
transfer-part-event-value = Part {$part} · offset {$offset} · {$length} · {$state} · attempt {$attempt} · connection {$connection} · {$elapsed}
settings-transfer-soft-limit-title = Telegram soft-limit policy
settings-transfer-soft-limit-description = Choose how the adaptive controller handles conservative Telegram guidance when measured goodput favors more active work.
settings-transfer-soft-limit-respect = Respect
settings-transfer-soft-limit-adaptive = Adaptive Override
settings-transfer-soft-limit-ignore = Ignore
settings-transfer-soft-limit-note = This policy controls advisory active-file limits. Select download speed behavior separately under Download strategy. Conflicts are recorded in Live/Replay and the session log; Telegram protocol limits and FLOOD_WAIT remain mandatory.

transfer-actions = Actions
transfer-show-details = Details
transfer-close-details = Close details
transfer-scope-visible = Current list
transfer-filter-count = { $label } · { $count }
transfer-delete-confirmation = Delete { $count } tasks? Downloaded files will be kept. Task history, logs and partial downloads will be removed.
transfer-filter-title = Filters
transfer-actions-applying = Applying task actions…

settings-download-strategy-title = Download strategy
settings-download-strategy-balanced = Balanced
settings-download-strategy-max = Max Throughput
settings-download-strategy-description = Max Throughput quickly probes up to 64 parallel parts and tolerates transient errors. It can use more bandwidth and memory; server wait times remain mandatory. Applies when a native download starts or resumes. Encrypted Vault transfers are unchanged.

account-welcome = Your files. Your private space.

account-welcome-back = Welcome back to TeleArk

account-login = Log In

account-switch = Switch Account

account-restoring = Restoring your Telegram session…

account-change-method = Use another login method

account-private-note = Your Telegram session stays on this Mac.

account-switch-description = Switching signs out of Telegram and pauses downloads. Files and progress are kept.

account-switching = Pausing downloads and signing out…

account-switch-busy = Wait for the current encrypted transfer to finish before switching accounts.

shell-preview = Preview

shell-utilities = Utilities

shell-local-library = Local Library

shell-account = Account

shell-disk-summary = { $free } free · { $used } used by TeleArk

vault-unlock-action = Unlock Vault


unlock-return-note = Unlock for this session to browse files, review notices, and choose your next action.

unlock-use-recovery = Use a recovery key

unlock-recovery-saved = I saved my recovery key — continue

storage-nav-title = TeleArk

storage-private-label = Your dedicated private channel

storage-remote-description = Created and managed by TeleArk. Do not edit this channel or delete it or its messages: this can break file access and recovery. Manage files through TeleArk.

storage-setup-title = Preparing your private storage

storage-setup-description = TeleArk discovers and verifies your storage directly on Telegram, on every device. It checks the channel’s identity before use. Conflicting or damaged identifiers stop setup; no channel is chosen arbitrarily.



storage-loading = TeleArk is finding or preparing your private channel…


storage-setup-error = TeleArk could not finish preparing the channel. Temporary errors are rechecked automatically; it will check again when you return. Existing files remain unchanged.

storage-locked-title = Unlock to view encrypted files

storage-guide-title = How TeleArk storage works

storage-guide-done = Got it

storage-guide-private-title = 1. A channel just for you

storage-guide-private-body = TeleArk manages one private storage channel per account. Do not edit or delete the channel or its messages in other apps: files may become unreadable or unrecoverable. Manage files through TeleArk.

storage-guide-files-title = 2. Work with complete files

storage-guide-files-body = Upload a file and TeleArk encrypts and splits it as needed. Unlock the vault to authenticate its manifest and see its original name. Downloads decrypt and verify before opening.

storage-guide-raw-title = 3. Inspect Raw Files

storage-guide-raw-body = Raw Files shows the Telegram objects, including ordinary uploads, encrypted pieces, and manifests. A recognizable name alone does not prove authenticity. Keep all pieces needed for recovery.

storage-guide-key-title = 4. Save your recovery key

storage-guide-key-body = Keys are prepared automatically. Keep a recovery bundle outside this Mac; Telegram cannot recover your keys. Recovery tools are in Settings → Encryption keys.

storage-legacy-title = Legacy Recovery

storage-legacy-description = Recover existing TeleArk files from Saved Messages. New uploads use your private channel.

storage-legacy-action = Recover from Saved Messages…

settings-about = About

about-description = A private home for files, built for your Mac.

about-changelog-title = What’s New

about-licenses = Open-source licenses

about-alpha = Pre-release · encrypted formats are awaiting independent security review.

menu-application-about = About TeleArk

menu-application-settings = Settings…

menu-view-transfers = Transfers

menu-view-storage = TeleArk Storage

menu-file-upload = Upload Encrypted File…

about-changelog-v040 =
    ## 0.4.0 · A new home for your files

    ### Made for the Mac
    - Rebuilt the desktop layer with GPUI Kit 0.6 and the matching gpui-pre family. Native typography, quiet surfaces, consistent controls, original vector symbols, and light/dark appearance.
    - Transfers and TeleArk storage stay at the top of the sidebar. Channels scroll independently, and refreshing never changes your current page or selection.
    - A centered account screen restores your avatar and name, with Log In and Switch Account. New sessions offer phone, QR, code, and two-step verification.
    - Unlock from the place that needs a key and continue your upload, download, or browse action. Changing pages no longer immediately locks the vault.
    - Unlock dialogs support Tab navigation, Return submission, Escape dismissal, and readable text at the smallest window size.

    ### Your own private channel
    - Create or rediscover a private channel owned by your Telegram account, with a distinct TeleArk destination in the sidebar.
    - Files shows authenticated manifests as complete logical files. Raw Files exposes ordinary files, encrypted parts, and manifests with their original names and metadata.
    - A built-in guide explains channel ownership, encryption, raw objects, recovery keys, and why manifests and parts must be kept.
    - Existing files in Saved Messages remain recoverable through Settings → Key Vault. New encrypted uploads target the private channel.

    ### Everything in its place
    - Transfer selection, batches, pause, resume, cancel, retry, safe deletion, file inspection, and live/replay diagnostics remain available. Advanced controls and details are tucked away until needed.
    - Settings groups daily preferences clearly and collapses less-used controls. Local import, search, file actions, indexing, storage paths, throughput strategies, and the full key lifecycle remain accessible.
    - Added native About and Settings menu items plus keyboard shortcuts for transfers, storage, search, refresh, and upload. About includes this complete release record.
    - Updated English, Simplified Chinese, and Japanese together.

    ### Reliability and compatibility
    - Native download history now records its Telegram account. Switching pauses active downloads and keeps progress; another account cannot resume or retry them.
    - Schema v9 preserves older history. Tasks without an account bind only when restoring the existing session, never to an arbitrary new login.
    - Private-channel discovery validates ownership, privacy, and the description marker. It preserves renamed bindings and asks you to choose when multiple candidates exist.
    - Simplified contributor documentation while preserving format specifications, security constraints, and architectural decisions. Git checkpoints preserve the documentation before and after consolidation.
    - Encrypted formats remain provisional pending independent security review. No live Telegram account is required by ordinary tests.

upload-choose-file = Choose Files…
upload-simple-description = TeleArk encrypts your file and its name before uploading. Large files are split automatically and restored as one file when downloaded.
transfer-manage = Manage

action-close-details = Close details
storage-scan-summary = { $count } files · { $rejected } unverified manifests skipped · Latest 1,000 manifests

shell-refresh-channels = Refresh channels

# Navigation, batches, and local filesystem observations
shell-expand-navigation = Expand navigation
shell-collapse-navigation = Collapse navigation
shell-free-disk-space = { $free } free
shell-disk-space-unavailable = Disk space unavailable
transfer-batch-progress = { $completed } / { $total } completed · { $failed } failed
transfer-batch-created = Added
transfer-expand-batch = Expand batch files
transfer-collapse-batch = Collapse batch files
local-file-present = Available locally
local-file-missing = Deleted from disk
local-file-size-changed = Local file changed
local-file-unavailable = Local file unavailable
local-file-checking = Checking local file

local-file-status = Local file
transfer-download-again = Download again

upload-selection-summary = { $count } files · { $size }
upload-remove-file = Remove file from selection
upload-stop-after-current = Stop after the current file
transfer-batch-upload-name = Upload · { $count } files

about-changelog-unreleased =
    ## 0.4.12 · Transfer table and key activity layout

    - ETA and Progress use wider responsive columns. A Telegram cooldown is shown once in the progress row instead of repeating in the rate slot.
    - Encryption key activity opens in a scrollable right-hand panel from the bottom status bar, replacing the full-width strip. On Windows, native full-screen keeps the status bar above the taskbar.
    - Application metadata is 0.4.12. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged; transfer, encrypted file and recovery codecs are unchanged.

    ## 0.4.11 · Native download recovery and diagnostics

    - Native download part retries switch to another transfer connection slot after a failure. A slot that reaches the 60-second request deadline is avoided for later parts of the same download, preventing one stalled slot from exhausting every attempt assigned to it.
    - The transfer inspector shows the measured part failure cause, part number, attempt, local connection slot and wait time. New version-2 part events retain this detail in the private session log so the final failure remains explainable after restart; older log records remain readable.
    - Application metadata is 0.4.11. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged. The native partial-file bitmap and encrypted file/recovery formats are unchanged.

    ## 0.4.10 · Configured Telegram builds and Windows MSI

    - Release CI validates the Telegram distribution API ID/hash secrets and passes them to every native compilation, including both universal macOS slices. Installed and portable apps use the same embedded configuration; reports never include the values.
    - Windows releases now include a native MSI alongside the standalone EXE and portable ZIP. The MSI supports in-place upgrades, repair, downgrade refusal and rollback, and automatically adopts previous per-user Inno installations while preserving unrelated files and app data. No extra SDK or runtime installation is required for users.
    - Application metadata is 0.4.10. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. The single workflow still targets Windows x64, universal macOS and Linux x64 and starts one release run per version tag.

    ## 0.4.9 · Correct Linux package verification

    - Linux release verification compares AppImage and portable archive executables with each other. Debian payloads are verified independently, allowing the expected binary changes made by their different packaging tools. Native installation, downgrade refusal and checksums remain checked.
    - CI tests the package comparison before compiling Rust, including corrupt and missing payload cases. Linux packaging and verification now appear as separate steps in the same workflow.
    - Application metadata is 0.4.9. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. Targets remain Windows x64, universal macOS and Linux x64; macOS packages remain unsigned and unnotarized.

    ## 0.4.8 · One release run per version tag

    - GitHub Actions now starts on release tags, pull requests and manual dispatch. Pushing `main` and a matching `v*` tag together starts one release run; direct branch pushes no longer trigger CI.
    - The release targets Windows x64, universal macOS (Intel and Apple Silicon), and Linux x64, with standalone executables, portable archives and native installers. The macOS and Linux packaging fixes from 0.4.7 remain included.
    - Application metadata is 0.4.8. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

    ## 0.4.7 · Focused release targets and packaging fixes

    - Tagged releases target Windows x64, universal macOS (Intel and Apple Silicon), and Linux x64. Each target provides a standalone executable, portable archive and native installer, for nine package files in the GitHub Release. Windows and Linux ARM64-only release jobs have been removed.
    - macOS verification now checks linked libraries in each architecture separately, avoiding a false failure on the universal binary's second heading. Debian packaging includes the copyright metadata required by `cargo-deb`. The three native installers retain in-place upgrades and downgrade refusal.
    - Application metadata is 0.4.7. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

    ## 0.4.6 · Unified CI and wider native releases

    - One GitHub Actions workflow runs quality and platform tests for branches and pull requests, and builds/publishes installers for matching `vX.Y.Z` tags. Its summaries show stage results, LF policy counts, Rust cache hits and release checksums. An optional manual package preview builds without publishing.
    - Windows x64 and ARM64, universal macOS (Apple Silicon and Intel), and Linux x64 and ARM64 now have native release packages. Windows uses a static CRT; macOS uses system frameworks; Linux AppImage and portable archives bundle linked runtime libraries. Debian installers resolve ordinary OS runtime packages automatically. No end-user build SDK is required.
    - Windows, macOS and Debian installers retain in-place upgrades and reject older versions before replacing files. Application metadata is 0.4.6; SQLite schema 22 and supported encrypted/recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

    ## 0.4.5 · Portable recovery and release installers

    - Encrypted uploads and downloads stream through bounded memory in 512 KiB transport blocks. New uploads do not create ciphertext spools; downloads write authenticated plaintext, and compatible older files remain readable.
    - Upload recovery can use authenticated remote receipts across a restart or a second device. Verified published containers are reused, while unpublished work restarts with fresh encryption identity. Transfer charts and timelines show measured activity with bounded history.
    - When a bound private channel is unavailable, management verifies remote state before discovering or creating a replacement. Existing local history and keys remain intact; files lost with the old remote channel are not restored.
    - Tagged releases build standalone executables, portable archives and native installers for Windows x86_64, macOS arm64 and Linux x86_64. Installers upgrade in place and reject an older version with an explanation. The workflow checks that the tag matches the application version before publication.
    - CI checks every tracked plain-text file for CRLF on Linux, Windows and macOS, rejects `.cmd` and `.bat` scripts, and reports counts and violations in the GitHub Actions summary. The release workflow applies the same policy before publication.
    - Updated application and macOS bundle metadata to 0.4.5. SQLite remains at read/write schema 22; supported older schemas migrate automatically. Encrypted file and recovery readers retain their documented compatibility. macOS packages remain unsigned and unnotarized.

    ## 0.4.4 · Session unlock and resilient background work

    - The storage page groups channel identification changes in a highlighted card with the exact message, pin and description updates. Locked-file cards keep their controls inside the border and scroll naturally at small sizes.
    - Vault unlock lasts for the account session. Explicit lock hides decrypted names and paths while admitted transfers, queued batches and synchronization continue; new operations require unlock. Upload unlock returns to confirmation.
    - Existing private-channel bindings remain fixed, with explicit management repair, file-local health and retained historical key versions. Proxy routing fails closed and applies across Telegram connections.
    - TeleArk setup separates instructions, the current task and state changes into distinct cards. Phase badges, response panels and action bars make waiting, retries and completion easy to distinguish.
    - Recent state changes are shown in timestamped rows, with an expandable bounded history. The independently scrolling inspector uses the same cards in English, Simplified Chinese and Japanese, in both themes.
    - Channel browsing uses local projections and background synchronization. Large libraries and transfer histories use bounded updates, indexed reads and background filesystem work to keep the window responsive.
    - Catalog server failures no longer overwrite the login state. Reads support cancellation and bounded retry, and incomplete discovery never authorizes private-channel creation. The observed Telegram catalog 500 error remains unresolved and also reproduces with the pre-performance code.
    - Database schemas 0–14 upgrade automatically to read/write schema 15, including skipped releases. Existing encrypted files, recovery bundles, sessions and transfer checkpoint codecs retain their supported versions.
    - Updated the application and macOS bundle to 0.4.4. Packaging remains unsigned; protected real-account and complete platform qualification remain outstanding.

    ## 0.4.3 · Upload activity at every step

    - Uploads now show storage checks, reading and encryption, waiting for Telegram, data transfer, remote confirmation, readback verification and manifest publication, with elapsed time and current object bytes.
    - Fixed progress remaining at zero until an entire encrypted part finished uploading and verification. In-flight progress updates as Telegram reads the byte stream; 100% is reserved for completed finalization.
    - Collapsed upload batches show the current member's activity. Queue rows appear before network preflight, and complete storage discovery runs once per batch while each file still rechecks its destination.
    - Private storage setup automatically verifies remote identity, preserves cross-device discovery, and explains why managed channel messages must be kept. Conflicting or damaged identities stop uploads.
    - New sign-ins start with QR login and offer a secondary phone method. Switching accounts requires confirmation and cannot interrupt pending uploads.
    - Library selection supports bulk reveal and account-scoped downloads. Newly uploaded files appear immediately, and older scans cannot replace their completion records.
    - Updated English, Simplified Chinese, Japanese and macOS bundle metadata. Database and encryption formats are unchanged; encrypted transfer controls and independent security qualification remain incomplete.

    ## 0.4.2 · Clearer Library, compact Transfers

    - Replaced All Files with Local files and Remote files tabs. Local files shows accessible downloads for the current account and imported originals; remote files shows the account's indexed Telegram catalog. File types have a separate filter.
    - Local file pages read current disk metadata, omit deleted or inaccessible files, and support cancellable paging and search without starting transfers.
    - Transfer files and batch rows now use the same compact 42-point height as Library rows. Upload and download arrows identify Transfers; batch details and controls remain available.
    - Fixed downloading again after deleting a local output: new tasks avoid paths retained by transfer history and preserve earlier records.
    - Updated English, Simplified Chinese, Japanese and macOS bundle version metadata. Schema and encryption formats are unchanged.

    ## 0.4.1 · Everyday usability

    - Library file details now offer account-scoped downloads for indexed Telegram files, with explicit guidance for unavailable sources. Local files retain Open and Reveal actions.

    - Fixed a channel pagination crash caused by re-entering the table update. Cancelled or stale requests cannot restart loading after navigation or account changes.
    - Local Library now explains indexed Telegram files, labels them as indexed instead of uploaded, and shows the original channel and scoped message ID. Removed the placeholder parts table.
    - Added a collapsible icon sidebar, direct Local Library access, a separate channel browser, and visible free space for the download disk.
    - Fixed scrolling in Raw Files and transfer inspectors so details scroll independently of the underlying list.
    - Batch rows now show their source, file names, counts, time, and real member lists. Multiple uploads can be reviewed together; stopping a batch skips remaining files after the current file finishes.
    - Downloaded files are checked in the background. Missing, changed, and unavailable files have distinct states; a missing native download can be downloaded again as a new task.
    - Added original TeleArk artwork and a macOS app bundle with an application icon.
    - Schema v10 preserves account-scoped local output identities across restarts. Encryption and manifest formats are unchanged.

file-state-remote-indexed = Indexed remote

library-catalog-explanation = This local catalog includes files seen while browsing or indexing Telegram, plus files you import. Indexed files are not upload or download records.

file-detail-source-record = Source record

file-detail-indexed-source-note = This metadata was saved from a Telegram source. It does not mean TeleArk uploaded or downloaded the file. The identifiers below locate the original message within its account and channel; current remote availability has not been rechecked.

file-detail-local-source-note = This file was imported from disk. Importing records metadata and does not upload its content to Telegram.

file-detail-source-account-id = Source account ID

file-detail-source-chat-id = Source channel ID

library-action-account-required = Download requires the Telegram account that indexed this file. Sign in to that account.
library-action-source-unavailable = This record has no unique downloadable message. Browse its source channel to locate the file.
library-action-managed-source = Open TeleArk → Files to unlock and restore this managed file.

library-tab-local = Local files
library-tab-remote = Remote files
library-types-all = All types
library-type-filter = Filter by file type
library-visible-files = { $count } files shown
library-visible-size = { $size } shown
library-local-explanation = Downloads from this account and imported files that are currently accessible on disk. Refresh to check again.
library-remote-explanation = Telegram files indexed for this account. Browse or index a channel to add files here; these records do not mean the files are downloaded.
library-local-empty-title = No local files found
library-local-empty-description = Download a file or import one from this computer. Deleted and inaccessible files are excluded; try clearing the search or type filter.
library-remote-empty-description = Browse or index a channel to add remote files, or clear the search and type filter.
file-detail-downloaded-source-note = This local copy was downloaded from the source below. Its current size and date come from the filesystem.

library-select-loaded = Select loaded files (up to 5,000)
library-select-file = Select file
library-selected-count = { $count } selected

account-switch-confirm-title = Switch account?
account-switch-confirm-description = Continuing will sign out the current account. You will need to sign in again to use it. Downloaded files and transfer history will be kept.
account-switch-confirm-action = Sign out and switch
account-use-phone = Log in with phone number
account-use-qr = Log in with QR code
account-qr-loading = Loading QR code…
account-qr-refreshing = Refreshing QR code after a sign-in error…
account-qr-refreshed = A new QR code is ready. Scan it to try again.
account-back-to-login = Back to sign in
account-qr-unavailable = QR code unavailable

storage-auto-created = TeleArk created your private storage channel. It is ready and will be managed automatically.

storage-auto-replaced = The previous private channel is no longer available. TeleArk connected a replacement channel and will manage it automatically. Files deleted with the old channel cannot be recovered; local copies remain unchanged.

storage-replacement-acknowledge = Got it

storage-auto-found = TeleArk found and connected your private storage channel. It is managed automatically.


storage-remote-title = 🔒 TeleArk · Managed Storage

storage-identity-conflict = TeleArk could not confirm one unique storage channel. It has stopped setup to avoid writing to the wrong channel. Existing channels and files are unchanged.

storage-identity-invalid = The channel’s remote identity or privacy settings do not match. TeleArk has stopped setup. Do not delete the channel or its files.

transfer-upload-preparing = Preparing the selected files for the upload queue…

transfer-upload-checking-storage = Checking storage channel
transfer-upload-checking-target = Checking upload destination
transfer-upload-reading-encrypting = Reading and encrypting
transfer-upload-sending-bytes = Uploading encrypted data
transfer-upload-confirming-message = Confirming remote message
transfer-upload-verifying-bytes = Reading back and verifying
transfer-upload-publishing-manifest = Publishing file manifest
transfer-upload-activity-elapsed = { $phase } · Elapsed { $elapsed }
transfer-upload-activity-bytes = { $phase } · { $done } / { $total } · Elapsed { $elapsed }
transfer-upload-activity-container-bytes = { $phase } · Current container: { $done } / { $total } · Elapsed { $elapsed }
transfer-upload-waiting-telegram = Waiting for Telegram

channel-sync-local-only = Local cache
channel-sync-queued = Sync queued
channel-sync-reading = Reading local cache
channel-sync-receiving = Receiving channel changes
channel-sync-persisting = Saving changes
channel-sync-verifying = Checking cached messages after a sync gap
channel-sync-waiting = Waiting to retry
channel-sync-idle = Listening for updates
channel-sync-failed = Sync needs attention
channel-sync-cancelled = Sync paused
channel-sync-history = Earlier history
channel-sync-empty = No files in the local cache yet. Background synchronization will update this view.
channel-sync-seeding = Preparing the initial local cache
channel-sync-history-loading = Receiving requested earlier history
channel-sync-details-title = Channel synchronization
channel-sync-rate-limited = Waiting for Telegram rate limit

startup-title = Opening your local library
startup-detecting = Checking the database version
startup-preparing = Preparing transaction protection for database version { $version }
startup-converting = Updating database structures to version { $version }
startup-verifying = Verifying database version { $version } before committing
startup-completed = Database ready · starting background services
startup-failed = The library could not be opened. Your existing data is retained. Check available disk space and access, then retry; newer databases require a compatible app version.
startup-truncated = Earlier startup events were removed from this bounded timeline.

managed-watch-title = Private channel changes
managed-watch-pending = Private channel changed · verifying
managed-watch-changed = Private channel: { $count } changes to review
managed-watch-acknowledge = Mark displayed changes as reviewed
managed-watch-retention = Recent changes shown · { $omitted } older records omitted
managed-watch-edited = Message edited
managed-watch-deleted = Message deleted
managed-watch-gap = Update gap · checking cached files
managed-watch-event = { $time } · { $kind } · message { $message }
managed-watch-gap-event = { $time } · { $kind }
managed-catalog-syncing = Synchronizing the private channel catalog. Available files will appear as manifests are verified.
managed-catalog-coverage = Initial sync covers up to 1,000 file manifests; new changes continue to sync.

managed-scan-queued = Manifest verification queued
managed-scan-reading = Reading file records
managed-scan-receiving = Waiting for Telegram manifests
managed-scan-verifying = Authenticating manifests
managed-scan-completed = Manifest verification complete
managed-scan-failed = Manifest verification failed
managed-scan-cancelled = Manifest verification cancelled
managed-sync-retry = File sync will retry automatically after a { $seconds }-second wait. You can keep using TeleArk.
managed-scan-unknown = unknown

transfer-session-log-omitted-label = Log gaps
transfer-session-log-omitted-count = All transfers this run: { $count } log records omitted

transfer-history-omitted = Older task records omitted from this list: { $count }
transfer-controller-history-omitted = { $count } earlier controller records omitted.
transfer-lifecycle-history-omitted = { $count } earlier state events omitted.
transfer-footer-total-retained = Shown: { $count }

telegram-error-server = Telegram's server could not process this request. Your login is still valid. Retry shortly.
dialogs-reading = Preparing workspace
dialogs-waiting = Workspace preparation · retrying shortly
dialogs-saving = Activating saved transfers
dialogs-complete = Workspace ready
dialogs-failed = Workspace needs attention · local data retained
dialogs-cancelled = Workspace preparation cancelled
dialogs-details = Workspace preparation

activity-state-queued = Queued
activity-state-running = In progress
activity-state-waiting = Retry scheduled
activity-state-saving = Saving
activity-state-complete = Complete
activity-state-failed = Needs attention
activity-state-cancelled = Cancelled
activity-last-response = Last response
dialogs-task-title = Workspace preparation
activity-history-title = State changes
activity-history-time-origin = Time since this run started · newest first
activity-history-show-all = Show all { $count } changes
activity-history-show-less = Show recent changes
storage-activity-title = Private storage
storage-activity-retry-wait = Telegram could not complete the check. Another attempt is scheduled.
activity-refresh = Refresh

settings-telegram-custom-enable = Enable custom API credentials
settings-telegram-custom-disable = Disable and use built-in credentials
settings-telegram-custom-purpose = Use this option to connect through the Telegram application registered to your own API ID and API Hash. Enter both values and save to apply them. Disabling removes the saved pair and restores the credentials bundled with TeleArk.

# Network proxy
proxy-settings-title = Network proxy
proxy-settings-description = Choose how TeleArk connects. Proxy connections use SOCKS5 or HTTP CONNECT and never fall back to direct access.
proxy-enable = Use proxy
proxy-disable = Direct connection
proxy-apply-note = Changes take effect after active transfers finish. You can cancel while waiting.
proxy-protocol-socks5 = SOCKS5
proxy-protocol-http = HTTP CONNECT
proxy-host = Proxy IP address
proxy-port = Port
proxy-username = Username (optional)
proxy-password = Password (optional)
proxy-address-note = Use an IPv4 or IPv6 address, not a hostname, to avoid DNS outside the proxy. Credentials are stored locally.
proxy-invalid = Check the IP address, port (1–65535), and credentials (at most 255 bytes each). A password requires a username; HTTP usernames cannot contain a colon. Changes were not applied.
proxy-apply = Apply and test
proxy-test = Test applied proxy again
proxy-phase-direct = Proxy disabled · direct access explicitly allowed
proxy-phase-ready = Proxy applied · no direct fallback
proxy-phase-applying = Applying network policy · closing old connections and saving
proxy-phase-connecting = Connecting through proxy · direct fallback blocked
proxy-phase-connected = Proxy tunnel connected
proxy-phase-testing = Testing applied proxy · establishing a Telegram TCP tunnel
proxy-phase-tested = Proxy tunnel test passed
proxy-phase-elapsed = Phase elapsed: { $elapsed }
proxy-test-result = TCP tunnel established in { $elapsed }. This tests proxy reachability, not account authorization.
proxy-error-unreachable = Cannot reach proxy. Direct access is blocked; check the proxy and retry.
proxy-error-timeout = Proxy connection timed out. Direct access remains blocked.
proxy-error-authentication = Proxy authentication failed. Check the username and password; direct access is blocked.
proxy-error-rejected = Proxy refused the tunnel. Direct access remains blocked.
proxy-error-protocol = Invalid proxy response. Direct access remains blocked.
proxy-error-disconnected = Proxy connection closed. Retry through the proxy; direct access remains blocked.
proxy-error-configuration = Proxy configuration is invalid. Networking is blocked.
proxy-error-persistence = Could not apply the network policy. Networking remains blocked; check local storage and retry.
proxy-error-capacity = Proxy connection limit reached. Direct access remains blocked.
proxy-load-failed = Network policy could not be read. Networking is blocked; check storage access or use a version compatible with this data.
proxy-timeline = Recent network events
proxy-copy-api-link = Copy API panel link
proxy-link-copied = Link copied. External browsers do not inherit TeleArk proxy settings.

proxy-test-cancelled = Proxy test cancelled. The proxy policy remains applied; no direct fallback.
proxy-cancel-test = Cancel test
proxy-history-expand = Show retained events
proxy-history-collapse = Show latest 8 events
proxy-timeline-truncated = Earlier events omitted: { $count }


proxy-phase-test-queued = Proxy test queued · waiting for its network slot

# Encrypted transfer failures (distinct from native downloads).
vault-transfer-error-target-permission = TeleArk could not verify permission to use the private storage channel. Check the active account and the channel’s ownership, privacy and identity record.
vault-transfer-error-permission = Access required by this encrypted transfer was denied. Check source-file access, local storage permissions and the private Telegram storage channel.
vault-transfer-error-invalid-request = The encrypted transfer request or package is invalid.
vault-transfer-error-conflict = The encrypted transfer conflicts with the current account, storage channel or local state.
vault-transfer-error-source-changed = The source or encrypted content changed, or failed integrity verification.
detail-failure-last-phase = Last recorded phase

# Bound channel resilience
storage-health-repair = The channel’s TeleArk identification message or its description reference is missing, invalid or no longer pinned.
storage-health-access = The bound channel is inaccessible or you no longer own it. Restore access and recheck.

storage-setup-phase-binding = Checking the saved channel binding
storage-setup-phase-dialogs = Reading the complete Telegram channel list
storage-setup-phase-verifying = Checking owner and privacy of the bound channel
storage-setup-phase-discovering = Looking for an available managed channel
storage-setup-phase-creating = Creating a private channel
storage-setup-phase-preparing = Verifying and preparing the managed channel
storage-setup-phase-saving = Saving the verified account/channel binding
storage-setup-phase-completed = Private channel ready
storage-health-unsafe = This channel must be private and have no other members. Correct its settings in Telegram, then recheck.
storage-health-unsupported = This channel uses a newer identity format. Update TeleArk; its data has been preserved.
storage-repair-action = Review channel changes
storage-repair-confirm = Repair this channel using the steps above? TeleArk will restore its identification message and description reference, then mute and archive the channel. All steps are required.
storage-maintenance-confirm = Confirm
storage-maintenance-time = Phase: { $seconds } s · Last activity: { $idle } s ago
storage-maintenance-omitted = { $count } earlier timeline events omitted
storage-maintenance-preview = Preview only — no Telegram changes made.
storage-repair-completed = Channel identification, pin and description reference are verified. The channel is muted and archived. Missing file data has not been restored.
storage-phase-checking = Checking account, binding and privacy
storage-phase-finding = Finding existing identity record
storage-phase-repairing = Restoring identity record
storage-phase-pinning = Pinning identity record
storage-phase-updating = Updating description pointer
storage-phase-verifying = Verifying remote changes and saving
storage-phase-muting = Muting channel
storage-phase-archiving = Archiving channel
storage-phase-completed = Completed and verified
vault-transfer-error-source-permission = The local source file cannot be read. Grant access or choose the file again.
vault-health-key-unavailable = The key for this file is unavailable. Import its recovery bundle in Settings → Encryption keys.

# File health and key epochs
vault-epoch-confirm-action = Create new key version
vault-epoch-lost-action = Generate a new upload key…
vault-epoch-confirm-description = Create a new key for future uploads in the same channel? Previous keys and encrypted files will be preserved. Files whose original key is missing still need their recovery bundle.
vault-epoch-preserved-description = Previous keys remain available for recovery. Restoring a historical key preserves the current upload key.
vault-epoch-historical-label = Original recovery bundle for an older key
vault-epoch-historical-action = Unlock historical key
vault-health-unchecked = Not checked
vault-health-present = All parts indexed
vault-health-missing-parts = Parts missing
vault-health-missing-manifest = Remote manifest missing
vault-health-key = Key unavailable
vault-health-invalid = Manifest damaged or unsupported
vault-health-detail = Health is based on synchronized message presence. File contents are authenticated during restoration. Missing parts affect only this file.
vault-health-key-version = Key version
vault-health-recheck = Recheck file health
vault-health-reupload = Upload local copy again
vault-health-unknown-size = Size unavailable until the key is recovered
vault-health-scope-limited = The view is limited to 1,000 records or 16 MiB. A full health check continues through paginated history and retained manifests.

vault-key-phase-queued = Waiting for key operation
vault-key-phase-generating = Generating a new key version
vault-key-phase-password = Preparing local key protection
vault-key-phase-recovery = Preparing recovery protection
vault-key-phase-saving = Preserving old keys and saving atomically
vault-key-phase-completed = Encryption keys ready
vault-key-phase-time = Phase: { $seconds } s · Last activity: { $idle } s ago
vault-key-details-title = Encryption key activity
vault-key-status-failed = Encryption key operation failed
vault-key-status-complete = Encryption key operation complete
vault-key-status-open = Show encryption key activity
vault-key-timeline-title = Phase timeline
vault-key-timeline-event = { $phase } · { $elapsed } from start
vault-health-check-summary = Last history check: { $count } files checked. Files with unavailable keys remain unchecked.
transfer-upload-saving-manifest = Saving the verified manifest locally

vault-session-locked-background = File encryption keys are unavailable. Telegram synchronization and already admitted work continue.
vault-session-unlock-policy = Unlock once per account session. Switching pages or leaving the window does not lock the Vault. Lock manually when needed; submitted tasks keep running.
vault-locked-file = Encrypted file — locked
vault-locked-detail = Unlock to view
upload-batch-still-running = The current batch is uploading. You can prepare the next batch and submit it when this one finishes.
storage-repair-title = Repair this TeleArk channel
storage-connected-title = Private channel connected
storage-repair-reason-missing = The TeleArk identification message or its reference in the channel description is missing.
storage-repair-reason-invalid = The referenced identification message does not match this account and channel.
storage-repair-reason-unpinned = The TeleArk identification message is no longer pinned.
storage-repair-explanation = TeleArk repairs this storage channel by applying all of these steps:
storage-repair-step-message = Reuse a valid TeleArk identification message, or post one if none is found.
storage-repair-step-pin = Pin that identification message in this channel.
storage-repair-step-description = Replace the channel description with TeleArk’s description and a reference to that message, then verify the result.
storage-repair-scope = File messages, the channel title, members and privacy settings stay unchanged. This does not restore missing file data.
storage-repair-confirm-action = Repair channel
storage-channel-options = Other channel actions
storage-recheck-action = Check channel again
storage-location-title = How TeleArk locates this channel
storage-location-target = Channel: { $title } · ID { $id }
storage-location-bound = This channel was found using the channel ID saved on this device for your current Telegram account. TeleArk then checked your ownership and the channel’s private configuration. A renamed channel or missing identification message does not change that saved destination; TeleArk does not choose a replacement by its name.
storage-location-method = TeleArk saves this channel’s ID separately for your Telegram account and uses it on later visits. Without a saved binding, it checks TeleArk identification in private channels you own; it connects only to a single verified match, or creates a private channel if none exists. Multiple matches require a choice. The channel name alone is not identification.
storage-notifications-title = Notifications and archived chats
storage-notifications-explanation = TeleArk mutes this channel’s notifications and moves it to Telegram’s Archived Chats as part of every channel repair. These are required defaults, with no separate switches. You can review the steps and choose whether to repair the channel. Archiving does not move or delete stored files.
storage-repair-step-archive = Mute channel notifications, move the channel to Archived Chats, and verify both settings.
upload-drop-files = Drop files anywhere in this upload window to add them, or choose files.
upload-files-independent = Each file is stored independently.
upload-folders-unsupported = Folders are not supported yet.
upload-selection-queued = Waiting for selection or the upload worker
upload-selection-checking-files = Checking file metadata
upload-selection-checking-channel = Checking the private channel
upload-selection-uploading = Uploading files
upload-selection-finished = Finished processing
upload-selection-cancelling = Stopping after the current operation finishes…
upload-selection-inspecting = Checked { $inspected } of { $total } selected paths.
upload-selection-counts = { $total } files · { $completed } uploaded · { $paused } paused · { $failed } failed · { $cancelled } cancelled · { $pending } not processed
upload-selection-timing = Current phase: { $elapsed } · Last activity: { $idle } ago
upload-selection-retention = Detailed history shows recent processing batches. These totals include the whole selection.
upload-selection-history-entry = { $time }: { $phase }

transfer-batch-open-window = Open batch in a separate window
transfer-batch-window-title = Batch files
transfer-batch-unavailable = This batch is no longer available in the current task history.
transfer-batch-window-live = Live progress · Closing this window keeps transfers running

transfer-history-restoring = Restoring upload history
transfer-upload-interrupted = Interrupted
transfer-upload-interrupted-reason = The app closed before this upload was confirmed complete.
transfer-upload-interrupted-action = Select the source file on the Upload page to start a new upload. Check Storage first if the interruption happened during publication.
transfer-history-restored-label = Restored history
transfer-history-restored-detail = Saved task totals are available. Live charts and detailed activity were not restored.

detail-vault-lifecycle-durable-upload = Task summary saved locally. Unfinished uploads are marked interrupted after restart; select the source file to start a new upload.
upload-error-folder = Folders and .app application bundles cannot be uploaded directly. Compress them into a ZIP file, then upload the ZIP.

speed-limits-title = Speed limits
speed-limits-description = Total cap per direction, in KiB/s. 1024 KiB/s = 1 MiB/s. Enter 0 for unlimited.
speed-limits-upload = Upload · KiB/s
speed-limits-download = Download · KiB/s
speed-limits-unlimited = Unlimited
speed-limits-scope = Applies to all file transfers, including encrypted files. Changes apply after saving. Payload pacing allows short buffered bursts; protocol overhead is excluded.
speed-limits-invalid = Enter non-negative whole numbers within the supported range.
speed-limits-save-failed = Could not save. The previous limits remain active. Retry Save.
speed-limits-ready = Save to apply to current and future transfers.
speed-limits-summary = Limits ↑ { $upload } · ↓ { $download } · Waiting: { $waiting } · { $seconds } s
speed-limits-budget = { $limit } · { $waiting } waiting for bandwidth · { $seconds } s
speed-limits-last-activity = Last payload admission: { $seconds } s ago
speed-limits-no-activity = No limited payload admitted yet.
speed-limits-history = Show / hide recent activity
speed-limits-event-changed = Limit changed
speed-limits-event-waiting = Waiting for bandwidth
speed-limits-event-resumed = Bandwidth wait ended
speed-limits-event = { $event } · { $seconds } s ago
speed-limits-omitted = { $count } older events omitted; latest 16 retained per direction.
speed-limits-close = Close

global-sync-connecting = Waiting for account connection
global-sync-discovering = Updating channel directory
global-sync-account = Account

global-sync-library = Updating file library
global-sync-library-failed = File library update needs attention


# Fixed synchronization event timestamps
sync-last-completed = Last completed
sync-no-completion = No completed sync yet
sync-event-time = Event time: { $time }
sync-event-completed = Synchronization complete
sync-file-verification = File verification
sync-recent-events = Recent activity
sync-no-events = No events yet
sync-older-events = Some older activity is no longer shown.
sync-event-times-local = Event times · local time

sync-silence-policy = Recovery check after { $minutes } minutes without channel updates.
channel-file-count = { $count } files
channel-select-all-compact = Select all
channel-download-compact = Download
shell-sync-complete = Synced
shell-sync-ready = Ready
shell-sync-active = Syncing
shell-sync-active-channel = Syncing 1 channel
shell-sync-active-channels = Syncing { $count } channels
shell-sync-active-attention = Syncing { $count } · needs attention
shell-sync-connecting = Connecting
shell-sync-waiting = Waiting to sync
shell-sync-paused = Sync paused
shell-sync-attention = Sync needs attention
shell-sync-details = View synchronization activity
shell-vault-locked = Encryption key needed

shell-sync-queued = Sync queued

transfer-upload-checking-source = Checking source for safe resume
transfer-upload-saving-recovery = Saving recovery information

transfer-upload-pausing = Pausing — saving confirmed work
transfer-upload-cancelling = Cancelling — stopping active work

transfer-recovery-unavailable = Recovery unavailable
transfer-recovery-unavailable-detail = The saved recovery information is damaged or uses an unsupported version. The original record and files have been preserved.
transfer-recovery-verification-pending = Saved progress will be checked locally before reuse.
transfer-recovery-saved-download = Saved download

transfer-recovery-retry-guidance = This task can be retried. Resolve the cause above, then choose Retry on its row. TeleArk checks saved work before reusing it; failed tasks do not retry automatically.
transfer-recovery-source-guidance = This upload cannot continue with the current source. Keep the original file and restore access if possible. When the file is ready, start a new upload from Upload; this stopped task remains in history.
transfer-recovery-key-guidance = Retry key access or import the file’s recovery bundle in Settings → Encryption keys. Then resume or retry the task. Keep the original source and any partial download.
transfer-recovery-blocked-guidance = This task cannot resume in its current state. Resolve the cause above before starting a new transfer. Keep the original source and any partial download; a new task may need to transfer the data again.
transfer-recovery-legacy-guidance = This saved task has no usable recovery action. Keep its record and files. A compatible app version may be needed for newer recovery data; otherwise start a new transfer. History alone does not guarantee resumable data.
storage-guide-transfers-title = 5. Pause and resume transfers
storage-guide-transfers-body = In Transfers, each task shows the actions it supports. Pause waits for active work to stop safely; Resume checks saved data before continuing. Keep upload sources unchanged and partial downloads in place. Cancel stops the task; it does not undo data already sent.
storage-guide-resume-title = 6. Understand recovery limits
storage-guide-resume-body = After restart and unlock, eligible queued transfers can continue. Paused tasks wait for Resume; failed tasks need attention. Open a task for its reason and next step. Older history or damaged recovery data may require a new transfer, and changes to sources, keys or Telegram files can prevent recovery.

upload-selection-saving-queue = Saving the upload queue
upload-selection-saved-count = Saved { $saved } of { $total } files to the upload queue.

native-cleanup-waiting = Waiting for download to stop
native-cleanup-removing = Cleaning temporary files
native-cleanup-failed = Temporary files need attention
native-cleanup-explanation = Cancellation is saved. Cleanup waits until the old download releases its files; completed files are kept.
native-cleanup-retry-waiting = Retry is saved and will start after temporary files are safely cleared.
native-cleanup-detail = { $reason } · Waiting: { $elapsed } · Last activity: { $last }
native-cleanup-finished = Temporary-file cleanup completed
native-cleanup-unsupported = The app cannot safely finish this cleanup. Temporary files are kept. Update the app before trying again.
native-cleanup-error-guidance = { $error } Temporary files are kept. Retry after resolving this issue; downloading starts only after cleanup succeeds.

settings-upload-tasks = Concurrent upload tasks
settings-upload-parts = Parallel 512 KiB parts per upload
settings-upload-connections = Upload connections
settings-upload-queue = Ready 512 KiB buffers per upload
settings-upload-attempts = Upload attempts per part
settings-download-tasks = Concurrent download tasks
settings-download-parts = Parallel parts per download
settings-download-connections = Download connections
settings-download-attempts = Download attempts per part
settings-transfer-manual-description = Settings apply when work starts or resumes. TeleArk keeps these values fixed; Telegram retry deadlines still apply.
settings-upload-resume-window = Resume within 24 hours; later attempts restart with a fresh file key. Telegram may expire temporary parts earlier. Interrupted local preparation restarts its container safely. Keep the source file unchanged until completion.
settings-aes-hardware-available = AES-256-GCM: hardware acceleration is available on this device.
settings-aes-hardware-unavailable = AES-256-GCM: software encryption on this device.
settings-tuning-increase = +
settings-tuning-decrease = −
transfer-reason-user-settings = User settings
transfer-upload-restarting-expired = The 24-hour upload window expired. Starting again with a fresh encryption identity.

transfer-upload-upgrading = Upgrading upload format with a fresh encryption identity

transfer-upload-sealing = Validating encrypted container
upload-pipeline-title = Upload activity
upload-pipeline-queue = Queued buffers: { $queued } · Active parts: { $active } · Last activity: { $idle } ago · Retry wait: { $wait }
upload-chart-title = Confirmed payload / second
upload-chart-empty = Waiting for measured acknowledgements. Speed is unknown.
upload-chart-sample = { $time } · { $speed } over { $interval }
upload-chart-range = { $from } – { $to } since this attempt started
upload-chart-explanation = Each sample uses the latest 3 seconds of confirmed payload. Samples publish once per second; the window edge is quantized to 50 ms. Silence expires to zero. File publication and integrity verification complete separately.
upload-part-map-title = Current container · 512 KiB upload parts
upload-part-map-legend = Gray: queued · Blue: sending · Amber: retry · Green: acknowledged. Last part may be shorter.
upload-part-state = Part { $part } · { $state } · Attempt { $attempt }
upload-part-queued = Queued
upload-part-active = Sending
upload-part-waiting = Waiting to retry
upload-part-confirmed = Acknowledged
upload-timeline-title = Activity timeline
upload-timeline-event = { $time } · { $phase } · Part { $part } · Attempt { $attempt } · Wait { $wait }
upload-history-omitted = Earlier history omitted: { $events } events, { $samples } samples.
upload-timeline-recent = Showing the latest 12 events. Replay opens earlier retained events.

upload-phase-restarting-unsealed = Restarting safely after interrupted manifest encryption

# Whole-application access gate and transfer-safe lifecycle actions.
app-pin-title = Application lock
app-pin-description = A 6–12 digit PIN protects access to TeleArk. Without a PIN, the app stays unlocked. File encryption is managed separately.
app-pin-enabled = On
app-pin-disabled = Off
app-pin-current = Current PIN
app-pin-new = New PIN · 6–12 digits
app-pin-confirm = Confirm PIN
app-pin-save = Save PIN
app-pin-disable = Disable PIN
app-pin-lock = Lock application
app-pin-checking = Checking PIN…
app-pin-saving = Verifying and saving PIN settings…
app-pin-saved = PIN settings saved
app-pin-incorrect = Incorrect PIN. Try again.
app-pin-format = Enter 6–12 digits.
app-pin-mismatch = The new PIN entries do not match.
app-pin-rate-limited = Too many attempts. Wait 30 seconds before trying again.
app-pin-save-failed = Could not save PIN settings. Existing protection is unchanged. Retry.
app-pin-load-failed = PIN settings could not be read. Restart to retry; the application remains locked.
app-lock-enter = Enter your PIN to sign in
app-lock-unlock = Sign in
app-lock-background = Sync, uploads and downloads continue in the background.
app-lock-back = Back to sign in
transition-quit = Quit TeleArk?
transition-account = Switch account?
transition-proxy = Apply proxy configuration?
transition-description = Wait for active transfers to finish before continuing.
transition-waiting = Background work continues. Cancel to keep using the current session.
transition-wait = Wait, then continue
proxy-fixed-timing = Phase started: { $phase } · Last event: { $activity }
proxy-event-time = { $time } · { $phase }
sync-log-open = Sync activity and logs
sync-private-event = { $kind } · Message { $message }
shell-sync-working-attention = Syncing · needs attention

app-lock-state = Locked
app-lock-switch = Switch account
transition-active = Transfers are running
transition-preserved = Uploads and downloads keep working. Paused tasks keep their saved progress.
transition-waiting-title = Waiting for transfers
transition-started = Requested at { $time }
sync-column-time = Time
sync-column-event = Activity
sync-log-description = Channel updates, private file sync and recent activity.

menu-window-close = Close Window

transition-quit-description = Pause uploads and downloads and save their progress before closing.

transition-quit-preserved = Saved tasks remain paused after you reopen TeleArk. File selection and unsaved preparation will stop.

transition-pause-quit = Pause and Quit

transition-pausing-title = Pausing tasks…

transition-pausing = Saving pause requests and waiting for active workers to close their files.

transition-pause-failed-title = Unable to finish pausing

transition-pause-failed = TeleArk is still open. Some tasks may already be paused. Check available disk space and try again.

transition-pause-retry = Retry Pause and Quit

session-loss-checking = Checking Telegram session…
session-loss-pausing = Telegram session ended · Pausing tasks…
session-loss-retiring = Tasks paused · Returning to sign in…
session-loss-failed = Session ended · Retry required
session-loss-paused = Telegram session ended · Tasks paused
session-loss-description = Your Telegram login is no longer valid. TeleArk will save task progress and return to sign in. If this step fails, retry to finish safely.
session-loss-step-time = Current step: { $duration } since last update
session-loss-history-omitted = { $count } earlier activity entries omitted

managed-key-title = Encryption keys

managed-key-description = Keys are generated automatically and protected by the system keychain. Your optional app PIN only locks TeleArk; it does not change file encryption.

managed-key-preparing = Preparing encryption keys…

managed-key-ready = Ready · Managed automatically

managed-key-unavailable = Encryption key needs attention

managed-key-unavailable-help = Retry key access in Settings → Encryption keys, or import a recovery bundle if the key is missing. Other pages remain available.

managed-key-retry = Retry key access

managed-key-recovery-options = Recovery options

managed-key-recovery-description = Keep a recovery bundle outside this device so you can recover encrypted files if the device or its keychain is lost. Importing an older key preserves the current upload key.

managed-key-show-recovery = View recovery bundle

managed-key-import = Import recovery bundle

managed-key-phase-loading = Reading the system keychain

managed-key-phase-securing = Saving and verifying the keychain entry

managed-key-store-error = The system keychain could not be accessed. Allow TeleArk access and retry in Settings → Encryption keys.

transfer-rate-sampling = Sampling…
detail-server-status = Telegram server status
transfer-persistence-parallel = { $activity } · Saving recovery information
transfer-eta-compact = ETA { $eta }
transfer-rate-basis = Confirmed application payload · 3 s window · updated every 1 s
transfer-rate-awaiting = Waiting for confirmation · last confirmation { $elapsed } ago

vault-health-pending-upload = Upload incomplete
vault-pending-resume = Continue upload…
vault-pending-select-source = Select the original file to continue uploading

transfer-download-receiving-blocks = Receiving and decrypting blocks

transfer-rate-awaiting-first = Waiting for first confirmation
transfer-bytes-heading = Processed / Total
transfer-eta-heading = ETA
upload-part-group = Blocks { $first }–{ $last } · { $confirmed } confirmed · { $state }
storage-channel-pending-explanation = This upload is incomplete. Restore its recovery key and select the same original file to continue; only published, verified containers can be reused.
upload-part-map-grouping = { $count } blocks · Up to { $size } per cell. Hover for exact ranges.
