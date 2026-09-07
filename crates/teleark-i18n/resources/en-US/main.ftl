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
settings-key-vault = Key Vault
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
nav-key-vault = Key Vault
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
detail-speed = Download Speed
detail-downloaded = Downloaded
detail-active-connections = Active Connections
detail-workers = Workers
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
vault-key-loss-warning = If both the password and recovery key are lost, encrypted Telegram files cannot be recovered.
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
telegram-api-id-open-settings-action = Open Settings
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
settings-telegram-credentials-description = Save the API ID and API Hash assigned to your own Telegram application. Both phone and QR sign-in require the complete pair.
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
settings-upload-vault-managed-description = TeleArk uploads encrypt content, names, and metadata with the unlocked Key Vault. The current safe plaintext part limit is { $size }.
upload-current-part-size = Current safe part limit
upload-current-part-size-description = TeleArk currently creates encrypted plaintext parts of at most { $size }.
settings-vault-title = Key Vault protection
settings-vault-description = Control when locally held encryption material is locked.
settings-vault-lock-when-hidden = Lock when TeleArk becomes inactive
settings-vault-lock-when-hidden-description = Switching pages keeps the vault unlocked. Switching away from the window locks it; running transfers may finish with their retained keys.
settings-vault-lock-now-action = Lock Key Vault now
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
detail-failure-retryable = This failure may be temporary. Check the conditions above and retry.
detail-failure-user-action = This failure needs a settings, account, source, or filesystem change before retrying.
detail-failure-terminal = This task stopped and will not retry automatically.
detail-verification-not-reached = Verification was not reached because the transfer failed first
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
vault-recovery-placeholder = Paste the complete recovery key
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
vault-error-invalid-request = Check the fields and make sure both password entries match.
vault-error-authorization = The Vault secret is incorrect, or the Vault is locked.
vault-error-source-missing = The selected source or remote package is unavailable.
vault-error-permission-denied = TeleArk does not have permission to access that location.
vault-error-network = Telegram could not complete the Vault operation. Try again.
vault-error-not-found = The requested Vault or package was not found.
vault-error-conflict = A Key Vault is already configured.
vault-error-capacity = The operation exceeded a supported size or storage limit.
vault-error-cancelled = The Vault operation was cancelled.
vault-error-persistence = Vault data could not be safely read, verified, or saved.
upload-file-picker-prompt = Choose Files
upload-no-file-selected = No file selected
upload-select-file-description = Select a source file. Its original path is never uploaded.
storage-channel-managed-vault-locked = Unlock to verify your files and reveal their original names. Raw Files is available without unlocking.
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
detail-integrity-teleark = BLAKE3 plaintext/ciphertext digests; 8 MiB authenticated frames
detail-manifest-codec = Manifest format
detail-manifest-codec-value = teleark-manifest-v1 (provisional)
transfer-preview-upload-note = Upload preview; the retained desktop encrypted-upload worker is not connected yet.
transfer-summary-uploading = Uploading
action-load-more = Load More
transfer-controller-title = Adaptive transfer controller
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
transfer-part-inflight = Inflight
transfer-part-completed = Completed
transfer-part-retry = Retry
transfer-part-failed = Failed
transfer-part-event-value = Part {$part} · offset {$offset} · {$length} · {$state} · attempt {$attempt} · {$elapsed}
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

vault-lock-action = Lock Vault

unlock-return-note = Unlock once to continue your action. Your keys stay in memory while TeleArk is active.

unlock-use-recovery = Use a recovery key

unlock-recovery-saved = I saved my recovery key — continue

storage-nav-title = TeleArk

storage-private-label = Your dedicated private channel

storage-remote-description = Private encrypted file storage managed by TeleArk. Keep this channel and its files for recovery.

storage-setup-title = A home for your files

storage-setup-description = Create a private Telegram channel owned by your account. TeleArk will store encrypted files here and recognize them automatically. Your Saved Messages stay yours.

storage-create-action = Create Private Channel

storage-discover-action = Find an existing TeleArk channel

storage-loading = Checking your private storage…

storage-unavailable = The saved channel is unavailable. Refresh or choose an existing TeleArk channel below. TeleArk will not silently replace it.

storage-setup-error = The channel could not be verified. Refresh to check again; an interrupted creation may already have succeeded.

storage-locked-title = Your files are safely locked

storage-guide-title = How TeleArk storage works

storage-guide-done = Got it

storage-guide-private-title = 1. A channel just for you

storage-guide-private-body = You own this private channel. TeleArk adds no members or public links. Keep its description marker so it can be found after reinstalling.

storage-guide-files-title = 2. Work with complete files

storage-guide-files-body = Upload a file and TeleArk encrypts and splits it as needed. Unlock the vault to authenticate its manifest and see its original name. Downloads decrypt and verify before opening.

storage-guide-raw-title = 3. Inspect Raw Files

storage-guide-raw-body = Raw Files shows the Telegram objects, including ordinary uploads, encrypted pieces, and manifests. A recognizable name alone does not prove authenticity. Keep all pieces needed for recovery.

storage-guide-key-title = 4. Save your recovery key

storage-guide-key-body = Keep the recovery bundle somewhere safe outside this Mac. Telegram cannot recover your encryption keys. Advanced recovery tools are in Settings → Key Vault.

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
upload-batch-limit = Choose up to 128 files. Each file remains an independent TeleArk file.
upload-remove-file = Remove file from selection
upload-stop-after-current = Stop after the current file
transfer-batch-upload-name = Upload · { $count } files

about-changelog-unreleased =
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
