## TeleArk 简体中文资源。

common-search = 搜索
common-filter = 筛选
common-view = 视图
common-upload = 上传
common-new-collection = 新建集合
common-cancel = 取消
common-save = 保存
common-pause = 暂停
common-resume = 继续
common-retry = 重试
common-delete = 删除
common-open-file = 打开文件
common-open-folder = 打开文件夹
common-more = 更多
common-status = 状态
common-size = 大小
common-type = 类型
common-source = 来源
common-modified = 修改时间
common-encrypted = 已加密
common-parts = 分片
common-details = 详情
common-all = 全部
common-connected = 已连接
common-account-count = { $count } 个账号

menu-application-services = 服务
menu-application-quit = 退出 TeleArk
menu-view-title = 显示
menu-view-toggle-fullscreen = 切换全屏幕
menu-window-title = 窗口
menu-window-minimize = 最小化
menu-window-zoom = 缩放

library-title = 文件库
library-all-files = 全部文件
library-recent = 最近新增
library-videos = 视频
library-documents = 文档
library-archives = 压缩包
library-audio = 音频
library-images = 图片
library-disk-images = 磁盘映像
library-other = 其他
library-channels = 频道
library-collections = 集合
library-storage-channel = TeleArk
library-transfers = 传输
library-uploads = 上传
library-downloads = 下载
library-waiting = 等待中
library-completed = 已完成
library-failed = 失败
library-search-placeholder = 搜索文件名、说明、标签或频道
library-item-count = 共 { $count } 项
library-column-name = 名称
library-local-storage = 本机存储
library-telegram-storage = Telegram 存储
library-storage-used = 已使用 { $used}，共 { $total }
library-empty-title = 暂无文件
library-empty-description = 索引频道或上传文件以开始构建文件库。
library-empty-description-local = 从这台电脑导入文件，以开始建立持久保存的本机文件库。
library-collection-preview-title = 集合预览
library-collection-preview-description = 此预览集合尚未连接到已保存的文件库。请选择文件库类别以浏览真实文件。
library-loading-title = 正在加载文件库
library-loading-description = TeleArk 正在读取本机文件库索引。
library-error-title = 无法加载文件库
library-result-count-dynamic = 共 { $count } 个结果
library-total-files-dynamic = 共 { $count } 个文件
library-local-index-size = 本机已索引 { $size }
library-first-page-note = 当前显示第一页
library-load-more = 加载更多
library-loading-more = 正在加载…
library-load-more-failed = 无法加载更多文件，请重试。
library-source-local = 这台电脑
library-source-telegram-chat = Telegram 聊天 { $chat_id }
library-file-picker-prompt = 导入
library-choosing-files = 正在选择文件…
library-importing-files = 正在导入文件…
library-import-success = 已导入 { $count } 个文件。
library-import-partial = 已导入 { $imported } 个文件；另有 { $failed } 个文件无法导入。
library-import-failed = { $count } 个文件无法导入。
library-import-empty = 未选择任何文件。
library-picker-failed = 无法打开系统文件选择器。

action-import-files = 导入文件

error-library-invalid-request = 此文件库请求无效。
error-library-not-found = 请求的文件库项目已不存在。
error-library-conflict = 此文件已存在，或与现有项目冲突。
error-library-persistence = TeleArk 无法读取或保存本机文件库。
error-library-source-missing = 找不到所选的源文件。
error-library-source-changed = 导入期间，所选源文件发生了变化。
error-library-permission-denied = TeleArk 没有访问此位置的权限。
error-library-capacity = 文件库无法接受此操作的更多数据。
error-library-authorization = 完成此操作需要授权。
error-library-network = 网络连接已中断。
error-library-cancelled = 操作已取消。
error-library-unknown = 文件库操作意外失败。

file-detail-empty-title = 请从文件库中选择一个文件
file-detail-empty-description = 导入文件并将其选中，即可查看已保存的详细信息。
file-detail-modified-at = 修改时间
file-detail-verification-unavailable = 此目录未保存已验证的分片或内容哈希证据。

upload-dialog-title = 上传到 TeleArk
upload-target-account = 目标账号
upload-target-channel = 目标频道
upload-storage-method = 存储方式
upload-automatic-multipart = 自动分片
upload-automatic-description = 自动将文件拆分为兼容的 { $size } 分片。
upload-part-size = 分片大小
upload-compatibility-mode = 1900 MiB 兼容模式
upload-part-size-mib = { $size } MiB
upload-custom-part-size = 自定义分片大小
upload-security = 安全
upload-client-encryption = 客户端加密
upload-encryption-profile = 加密配置
upload-hide-filename = 隐藏原始文件名
upload-encrypt-metadata = 加密文件名、大小和类型等元数据
upload-estimate = 预计信息
upload-part-count = 分片数量：{ $count }
upload-total-size = 总大小：{ $size }
upload-telegram-messages = Telegram 消息数：{ $count }
upload-estimated-time = 预计时间：{ $time }
upload-change-file = 重新选择
upload-add-to-queue = 加入上传队列
upload-select-file = 选择文件

transfer-title = 传输
transfer-all-tasks = 全部任务
transfer-uploads = 上传
transfer-downloads = 下载
transfer-waiting = 等待中
transfer-completed = 已完成
transfer-failed = 失败
transfer-start-all = 全部开始
transfer-pause-all = 全部暂停
transfer-retry-failed = 重试失败任务
transfer-clear-completed = 清空已完成
transfer-new-queue = 新建队列
transfer-settings = 传输设置
transfer-downloading-count = 下载中：{ $count }
transfer-waiting-count = 等待中：{ $count }
transfer-completed-count = 已完成：{ $count }
transfer-failed-count = 失败：{ $count }
transfer-total-speed = 总速度：{ $speed }
transfer-today-transferred = 今日传输：{ $size }
transfer-state-queued = 已排队
transfer-state-uploading = 上传中
transfer-state-downloading = 下载中
transfer-state-paused = 已暂停
transfer-state-waiting-retry = 等待重试
transfer-state-verifying = 正在校验
transfer-state-completed = 已完成
transfer-state-failed = 失败
transfer-state-cancelled = 已取消
transfer-eta = 预计完成：{ $time }
transfer-remaining-time = 剩余时间：{ $time }
transfer-parts-completed = 已完成 { $completed } / { $total } 个分片
transfer-queue-stats = { $running } 个运行中，{ $waiting } 个等待中

file-detail-title = 文件详情
file-detail-overview = 概览
file-detail-parts = 分片
file-detail-details = 详情
file-detail-activity = 活动记录
file-detail-uploaded-verified = 已上传 · 已校验
file-detail-file-name = 文件名
file-detail-size = 大小
file-detail-type = 类型
file-detail-source = 来源
file-detail-uploaded-at = 上传时间
file-detail-part-count = { $count } 个分片
file-detail-encryption-status = 加密状态
file-detail-encryption-profile = 加密配置
file-detail-file-hash = 文件哈希
file-detail-part-number = 分片
file-detail-telegram-message-id = Telegram 消息 ID
file-detail-open-location = 打开文件位置
file-detail-more-actions = 更多操作

vault-title = 密钥库
vault-key-management = 密钥管理
vault-personal-master-key = 个人主密钥
vault-unlocked = 已解锁
vault-locked = 已锁定
vault-created-at = 创建于 { $date }
vault-kdf = 密钥派生
vault-status = 状态
vault-change-password = 更改密码
vault-lock-vault = 锁定密钥库
vault-recovery-backup = 恢复与备份
vault-recovery-key = 恢复密钥
vault-backed-up = 已备份
vault-not-backed-up = 未备份
vault-show = 显示
vault-export = 导出
vault-warning-key-loss = 重要：如果密码和恢复密钥均丢失，存储在 Telegram 中的加密文件将无法恢复。

index-title = 频道索引
index-status = 索引状态
index-messages-scanned = 已扫描消息：{ $count }
index-files-indexed = 已索引文件：{ $count }
index-indexed-size = 已索引容量：{ $size }
index-latest-sync = 最近同步：{ $date }
index-new-files = 新文件：{ $count }
index-current-progress = 进度：{ $percent }
index-current-date = 当前日期：{ $date }
index-scan-speed = 扫描速度：{ $speed }
index-eta = 预计剩余时间：{ $time }
index-start = 开始索引
index-pause = 暂停索引
index-resume = 继续索引
index-cancel = 取消索引
index-coverage = 索引覆盖范围
index-coverage-complete = 完整
index-coverage-partial = 部分
index-coverage-not-scanned = 未扫描
index-last-30-days = 最近 30 天
index-last-6-months = 最近 6 个月
index-last-year = 最近 1 年
index-all-history = 全部历史记录
index-custom-range = 自定义日期范围
index-content-types = 内容类型
index-files = 文件
index-videos = 视频
index-images = 图片
index-audio = 音频
index-plain-text = 纯文本消息
index-skip-small-media = 跳过小于 { $size } 的媒体

settings-title = 设置
settings-general = 通用
settings-accounts = 账号管理
settings-storage = 存储设置
settings-downloads = 下载设置
settings-uploads = 上传设置
settings-key-vault = 密钥与密码
settings-index = 索引设置
settings-notifications = 通知
settings-appearance = 外观
settings-advanced = 高级
settings-language-title = 语言
settings-language-description = 选择 TeleArk 的界面语言。
settings-language-system-default = 跟随系统
settings-language-english = 英语
settings-language-chinese = 简体中文
settings-language-japanese = 日语
settings-theme = 主题
settings-theme-system = 跟随系统
settings-theme-light = 浅色
settings-theme-dark = 深色
settings-concurrency = 并发传输数
settings-bandwidth = 带宽限制

error-transfer-network = 网络连接已中断。请检查连接后重试。
error-transfer-flood-wait = Telegram 要求 TeleArk 等待 { $seconds } 秒后再重试。
error-transfer-authorization = 此 Telegram 账号无权执行所请求的传输。
error-transfer-source-missing = 找不到源文件。
error-transfer-source-changed = 传输开始后源文件发生了变化。请重新上传。
error-transfer-disk-full = 目标位置没有足够的可用空间。
error-transfer-permission-denied = TeleArk 没有访问此位置的权限。
error-transfer-remote-missing = 缺少所需的 Telegram 远程对象。
error-transfer-hash-mismatch = 文件哈希不匹配，完整性校验失败。
error-transfer-authentication-failed = 加密内容认证失败。
error-transfer-manifest-corrupted = 软件包清单已损坏或无效。
error-transfer-unsupported-manifest = 当前版本的 TeleArk 不支持清单版本 { $version }。
error-transfer-key-unavailable = 无法获取此文件所需的加密密钥。
error-transfer-wrong-password = 密钥库密码不正确。
error-transfer-database = TeleArk 无法保存传输状态。重试可能会解决此问题。
error-transfer-cancelled = 传输已取消。
error-transfer-unknown = 传输因未知错误而失败。

## GPUI 参考界面使用的语义 ID。

search-placeholder = 搜索文件名、说明或频道
connection-connected = 已连接
accounts-count = 3 个账号
account-standard = 标准账号
channel-private = 私人频道
status-healthy = 正常
storage-local = 本机存储
storage-telegram = Telegram 存储
overall-title = 总览
overall-download-speed = 下载
overall-upload-speed = 上传
overall-free-space = 磁盘可用
overall-app-usage = 应用占用

action-cancel = { common-cancel }
action-upload = { common-upload }
action-download = 下载
action-filter = { common-filter }
action-start-all = 全部开始
action-pause-all = 全部暂停
action-resume-all = 全部继续
action-retry-failed = 重试失败任务
action-clear-completed = 清空已完成
action-delete-task = 删除任务
action-confirm-delete-task = 确认删除（保留文件）
action-new-queue = 新建队列
action-pause = { common-pause }
action-resume = { common-resume }
action-retry = { common-retry }
action-open-file = { common-open-file }
action-open-location = 打开位置
file-detail-local-path = 本地路径
collection-new = { common-new-collection }

nav-library = 文件库
nav-all-files = 全部文件
nav-recent = 最近新增
nav-videos = 视频
nav-documents = 文档
nav-archives = 压缩包
nav-channels = 频道
nav-collections = 集合
nav-transfers = 传输
nav-all-transfers = 全部传输
nav-completed = 已完成
nav-failed = 失败
nav-storage = 存储
nav-key-vault = 密钥库
nav-settings = 设置

filter-all-channels = 全部频道
filter-video = 视频
filter-large-files = 大于 1 GB
filter-all-statuses = 全部状态

table-name = 名称
table-size = 大小
table-type = 类型
table-source = 来源
table-modified = 修改时间
table-status = 状态
table-encrypted = 加密
table-parts = 分片
table-progress = 进度
table-speed = 速度
table-destination = 保存路径

library-result-count = 128,974 个结果
library-total-files = 共 128,974 个文件

file-type-video = 视频
file-type-disk-image = 磁盘映像
file-type-document = 文档
file-type-archive = 压缩包
file-type-image = 图片
file-type-audio = 音频
file-type-other = 其他
file-state-remote = 云端
file-state-downloaded = 已下载
file-state-uploaded = 已上传
file-state-verified = 已校验
file-state-encrypted = 已加密
file-state-local = 本机
file-state-uploading = 正在上传
file-state-verifying = 正在验证
file-state-verification-failed = 验证失败
file-state-remote-missing = 远程文件缺失
file-state-locked = 已锁定

transfer-state-waiting = 等待中
transfer-summary-downloading = 下载中
transfer-summary-task-count = { $count } 个任务
transfer-summary-completed-note = 已完成下载
transfer-summary-live-runtime = Telegram 实时传输运行时
transfer-summary-completed-data = 已完成数据量
transfer-value-unavailable = —
transfer-summary-tasks = 8 个任务
transfer-summary-waiting = 等待中
transfer-summary-ready = 等待开始
transfer-summary-completed = 已完成
transfer-summary-today = 今日 +128
transfer-summary-failed = 失败
transfer-summary-retry = 需要重试
transfer-summary-total-speed = 总速度
transfer-summary-today-data = 今日下载
transfer-summary-month-change = 比昨日增加 28%
transfer-footer-total = 共 166 个任务
transfer-footer-downloading = 下载中 8 个（78.27 GB）
transfer-footer-waiting = 等待中 156 个（210.43 GB）
transfer-footer-unlimited = 限速：无限制
transfer-footer-total-live = 共 { $count } 个任务
transfer-footer-downloading-live = { $count } 个下载中
transfer-footer-waiting-live = { $count } 个等待中
transfer-empty = 暂无传输任务。请先选择 Telegram 来源并下载文件。
transfer-batch-name = { $source } · { $count } 个文件
transfer-tab-log = 日志

connection-title = 连接
connection-address = IP 地址:端口
connection-client = 客户端
connection-latency = 延迟

detail-source-channel = 来源频道
detail-message-id = 消息 ID
detail-local-path = 本地路径
detail-speed = 下载速度
detail-downloaded = 已下载
detail-active-connections = 活跃连接数
detail-workers = 线程数
detail-retries = 重试次数
detail-created = 创建时间
detail-started = 开始时间
detail-time-remaining = 剩余时间
detail-tab-details = 详情
detail-tab-file-list = 文件列表
detail-verification = 文件校验

log-connected = 已连接到节点
log-block-downloaded = 已下载数据块
log-file-list = 已获取文件列表
log-verified = 下载完成，校验通过
log-connection-timeout = 连接超时，正在重试
log-saved = 下载完成，文件已保存

upload-source-unchanged = 源文件未发生变化
upload-source-checked = 刚刚检查
upload-automatic-multipart-description = 将此文件拆分为兼容 Telegram 的应用分片。
upload-conservative-mode = 1024 MiB 保守模式
upload-estimate-title = 预计信息
upload-estimate-parts = 分片数量
upload-estimate-part-size = 每片大小
upload-estimate-total-size = 总大小
upload-estimate-messages = Telegram 消息数
upload-estimate-time = 预计时间
upload-client-encryption-description = 上传前在本机加密文件内容。
upload-hide-filename-description = 使用受保护的名称代替原始文件名。
upload-encrypt-metadata-description = 保护文件名、大小和类型元数据。

file-detail-hash = 文件哈希
file-detail-part-size = 分片大小
file-detail-encryption = 加密算法
file-detail-frame-size = 加密帧大小
file-detail-manifest = 清单
file-detail-package-id = 软件包 ID
file-detail-tab-parts = 分片
file-detail-tab-details = 详情
file-detail-tab-activity = 活动记录
file-detail-all-parts-verified = 所有分片均已校验
file-detail-manifest-synced = 清单已同步
file-detail-part-index = 分片
file-detail-remote-id = Telegram 消息 ID
file-detail-upload-time = 上传时间

index-channel-title = 频道索引详情
index-options = 索引选项
index-status-synced = 已同步
index-channel-description = 支持增量同步和历史覆盖范围的本地元数据索引。
index-since-last-sync = 自上次同步以来
index-history-scan = 历史扫描
index-batch-size = 批量大小：1,000
index-estimated-time = 预计时间
index-until-complete = 距离完成
index-coverage-title = 历史索引覆盖范围
index-coverage-description = 已存储的范围会显示完整、部分和未扫描的历史记录。
index-range-complete = 完整范围
index-range-active = 活跃范围
index-range-unscanned = 未扫描范围
index-current-job = 当前索引任务
index-state-paused = 已暂停
index-state-indexing = 索引中
index-range-progress = 范围进度
index-job-range = 日期范围
index-job-checkpoint = 检查点
index-job-checkpoint-value = 消息 { $id }
index-job-files-found = 已发现文件
index-job-errors = 错误
index-job-updated = 最近更新
index-coverage-missing = 未扫描

settings-saved = 已保存
settings-indexing = 索引设置
settings-language-runtime-note = 语言更改会立即应用到所有已打开的 TeleArk 界面。
settings-language-persistence-ready = 语言偏好将保存在这台 Mac 上。
settings-language-persistence-saving = 正在保存语言偏好…
settings-language-persistence-saved = 语言偏好已保存
settings-language-persistence-failed = 无法保存语言偏好
settings-theme-system-description = 使用当前的系统外观设置。
settings-theme-light-description = 始终使用浅色外观。
settings-theme-dark-description = 始终使用深色外观。
settings-behavior-title = 应用行为
settings-start-at-login = 登录时启动 TeleArk
settings-start-at-login-description = 登录系统后自动打开 TeleArk。
settings-restore-window = 恢复上次窗口
settings-restore-window-description = 重新打开上次会话使用的界面。
settings-show-menu-bar = 显示菜单栏状态
settings-show-menu-bar-description = 在系统菜单栏中显示传输状态。

vault-master-key = 个人主密钥
vault-status-locked = 已锁定
vault-status-unlocked = 已解锁
vault-created = 创建时间
vault-cipher = 内容加密算法
vault-unlock = 解锁密钥库
vault-lock = 锁定密钥库
vault-recovery-title = 恢复密钥
vault-recovery-backed-up = 已备份
vault-show-recovery = 显示恢复密钥
vault-hide-recovery = 隐藏恢复密钥
vault-export-recovery = 导出恢复密钥
vault-profile-title = 默认加密配置
vault-option-hidden-filenames = 隐藏文件名
vault-option-hidden-filenames-description = 不在 Telegram 中暴露原始文件名。
vault-option-encrypted-metadata = 加密元数据
vault-option-encrypted-metadata-description = 保护文件名、大小和文件类型。
vault-option-compatible-parts = 兼容分片
vault-option-compatible-parts-description = 使用 1900 MiB 应用分片。
vault-option-keychain = 系统凭据存储
vault-option-keychain-description = 将包装密钥保存在操作系统钥匙串中。
vault-key-loss-warning = 如果密码和恢复密钥均丢失，Telegram 中的加密文件将无法恢复。
vault-unlock-to-view = 解锁密钥库后查看

prototype-demo-badge = 预览版
common-not-applicable = 不适用
file-detail-not-encrypted = 未加密
file-detail-parts-pending = 分片等待校验
file-detail-manifest-pending = 清单等待同步
action-start = 开始
table-eta = 预计完成
detail-transferred = 已传输
detail-verification-passed = BLAKE3 校验通过
detail-verification-pending = 等待校验
detail-verification-failed = 校验失败
settings-session-only = 仅本次会话
settings-preview-controls = 其他设置为预览功能
action-view-options = 视图选项
action-back = 返回
action-more = 更多操作
telegram-library-title = Telegram 来源
telegram-status-working = 正在处理…
telegram-status-failed = 需要处理
telegram-status-connected = 已连接
telegram-status-not-connected = 未连接
telegram-header-login-action = 登录
window-exit-fullscreen-action = 退出全屏
telegram-login-title = 登录 Telegram
telegram-login-description = 选择下方任一种登录方式。仅当账号启用两步验证时，Telegram 才会在验证码之后要求输入密码。
telegram-phone-login-title = 使用手机号登录
telegram-phone-login-description = 输入手机号以接收登录验证码。TeleArk 将使用“设置”中保存的 API 凭据。
telegram-api-id-label = API ID
telegram-api-hash-label = API Hash
telegram-phone-label = 手机号
telegram-api-id-placeholder = 数字 API ID
telegram-api-hash-placeholder = API Hash
telegram-credentials-prompt-title = 设置 Telegram API 访问
telegram-credentials-prompt-description = 输入你自己的 API ID 和 API Hash，以启用 Telegram 登录、频道浏览和下载。你可以暂时跳过，之后在“设置”中添加。
telegram-credentials-save-action = 保存凭据
telegram-api-id-skip-action = 暂时跳过
telegram-api-id-required-title = Telegram 登录已停用
telegram-api-id-required-description = 请在“设置”中保存你自己的 API ID 和 API Hash，以启用手机号和二维码登录。TeleArk 不会使用共享的 Telegram Desktop 凭据。
telegram-api-id-open-settings-action = 打开设置
telegram-credentials-official-panel-note = 请在 Telegram 官方 API 开发面板中创建和管理应用凭据。如果 API Hash 丢失，请返回面板管理或重新创建应用凭据。
telegram-phone-placeholder = 国际格式手机号
telegram-code-placeholder = 登录验证码
telegram-password-placeholder = 两步验证密码
telegram-connect-action = 连接并发送验证码
telegram-connect-qr-action = 使用二维码登录
telegram-qr-title = 使用 Telegram 扫描
telegram-qr-description = 在 Telegram 手机应用中打开“设置 › 设备 › 连接桌面设备”，然后扫描此二维码。
telegram-qr-placeholder = 输入 API ID 和 API Hash 后生成安全登录二维码。
telegram-qr-credentials-placeholder = 需要 API 凭据
telegram-qr-refresh-note = 二维码过期后会自动刷新。请勿向他人展示此窗口。
telegram-qr-refresh-action = 立即刷新二维码
telegram-qr-use-phone-action = 改用手机号登录
telegram-code-title = 输入登录验证码
telegram-code-description = Telegram 已向你的账号发送验证码，请在下方输入。
telegram-code-action = 验证验证码
telegram-password-title = 两步验证
telegram-password-description = 输入 Telegram 两步验证密码。
telegram-password-hint = 密码提示：{ $hint }
telegram-password-action = 解锁账号
settings-telegram-credentials-title = Telegram API 凭据
settings-telegram-credentials-description = 保存你自己的 Telegram 应用对应的 API ID 和 API Hash。手机号和二维码登录都需要完整的凭据组合。
settings-telegram-credentials-save-action = 保存凭据
settings-telegram-credentials-clear-action = 移除已保存的凭据
settings-telegram-api-id-missing = 尚未配置
settings-telegram-api-id-configured = 已配置
settings-telegram-api-id-distribution = 内置发行凭据
settings-telegram-api-id-saving = 正在保存…
settings-telegram-api-id-saved = 已保存
settings-telegram-credentials-removed = 已移除
settings-telegram-credentials-removed-using-distribution = 已移除个人凭据，正使用内置发行凭据
settings-telegram-api-id-invalid = 请输入大于零的数字 API ID，以及由 32 个十六进制字符组成的 API Hash。
settings-telegram-api-id-failed = 无法保存凭据。请检查本地存储后重试。
settings-telegram-credentials-storage-note = 你的 API ID 和 API Hash 会存入 TeleArk 的本地 SQLite 资料库数据库。请保护好你的 macOS 账户和备份。个人凭据会覆盖 TeleArk 发行方随构建提供的凭据。
settings-telegram-credentials-distribution-note = 当前版本正在使用其 TeleArk 发行方注册的 API 凭据，因此无需额外设置即可登录。你可以在上方保存自己的应用凭据来覆盖它们。TeleArk 绝不会使用共享的 Telegram Desktop 凭据。
settings-telegram-api-panel-action = 打开 Telegram API 开发面板
settings-preferences-ready = 设置已存储在本机
settings-preferences-saving = 正在保存设置…
settings-preferences-saved = 设置已保存
settings-preferences-failed = 无法保存设置
settings-storage-library-title = 本地资料库数据库
settings-storage-library-description = 查看用于账号、索引、传输、检查点和偏好设置的 SQLite 数据库。
settings-path-unavailable = 路径不可用
settings-storage-reveal-action = 在访达中显示数据库
settings-storage-sqlite-note = 此数据库由 TeleArk 管理。复制备份前请关闭 TeleArk，且不要使用其他应用编辑它。
settings-download-title = 托管文件与下载
settings-download-description = TeleArk 将下载、缓存和诊断日志统一放在同一个位置。下载会自动开始，不再逐个询问保存位置。
settings-managed-root-label = TeleArk 托管文件位置
settings-managed-downloads-label = 下载目录
settings-managed-cache-label = 缓存目录
settings-managed-root-action = 选择托管文件位置
settings-managed-root-default-action = 使用默认位置
settings-managed-root-open-action = 打开托管文件夹
settings-managed-root-picker = 选择用于存放 TeleArk 下载、缓存和日志的文件夹
settings-download-directory-unset = 每次开始下载时询问保存位置
settings-download-directory-action = 选择下载文件夹
settings-download-directory-clear-action = 清除默认文件夹
settings-download-directory-picker = 选择默认下载文件夹
settings-download-ask-each-time = 每个文件都询问保存位置
settings-download-ask-each-time-description = 每次开始 Telegram 下载时显示保存对话框。关闭此项前请先选择默认文件夹。
settings-download-reveal-completed = 在访达中显示已完成的下载
settings-download-reveal-completed-description = 文件校验成功后自动在访达中显示。
settings-upload-title = 上传默认设置
settings-upload-description = 加密上传使用你的 TeleArk 私有频道。
settings-upload-vault-managed-title = 由密钥保管库管理的加密上传
settings-upload-vault-managed-description = TeleArk 上传使用已解锁的密钥库加密内容、文件名和元数据。当前安全的明文分片上限为 { $size }。
upload-current-part-size = 当前安全分片上限
upload-current-part-size-description = TeleArk 当前生成的加密明文分片最大为 { $size }。
settings-vault-title = 密钥库保护
settings-vault-description = 控制本地加密材料何时锁定。
settings-vault-lock-when-hidden = TeleArk 失去活动状态时锁定
settings-vault-lock-when-hidden-description = 切换页面会保持解锁状态。离开应用窗口时会锁定；正在运行的传输可能使用已持有的密钥完成。
settings-vault-lock-now-action = 立即锁定密钥库
settings-index-title = Telegram 索引
settings-index-description = 选择 TeleArk 每次频道索引请求扫描的消息数量。
settings-index-batch-option = { $count } 条消息
settings-index-batch-option-description = 每次扫描的消息数
settings-notification-title = 传输通知
settings-notification-description = 选择哪些下载结果显示为应用内通知。
settings-notify-download-completed = 下载完成
settings-notify-download-completed-description = 文件校验完成后显示成功通知。
settings-notify-download-failed = 下载失败
settings-notify-download-failed-description = 传输无法完成时显示错误通知。
settings-appearance-title = 外观
settings-appearance-description = 选择 TeleArk 窗口和控件的配色方式。
notification-download-completed = 下载完成：{ $name }
notification-download-failed = 下载失败：{ $name }
action-show-in-folder = 在访达中显示
transfer-footer-selected = 已选择 { $count } 项
telegram-channel-select-title = 选择来源
telegram-no-channel-selected = 未选择来源
telegram-index-description = 分页扫描此来源，并将发现的文件保存到本地可搜索资料库。
telegram-index-next-action = 扫描接下来的 1,000 条消息
telegram-index-complete = 扫描完成。共已索引 { $count } 个文件。
telegram-index-page-complete = 本页完成，保存了 { $count } 个文件。可继续扫描更早的消息。
telegram-files-title = 可下载文件（{ $count }）
telegram-files-selected = 已选择 { $count } 项
telegram-files-select-all = 全选筛选结果
telegram-files-clear-selection = 清除选择
telegram-files-refresh-action = 刷新文件
telegram-files-loading = 正在从此来源加载文件…
telegram-files-fetching-progress = 正在拉取新消息…已扫描 { $scanned } / { $target } 条
telegram-files-fetching-slow = Telegram 响应较慢，你可以取消后重试。
telegram-files-cancel-action = 取消
telegram-files-retry-action = 重试
telegram-files-load-failed = 无法加载新消息，请检查网络连接后重试。
telegram-files-empty = 当前页面中没有找到可下载的文档。
telegram-file-unnamed = Telegram 文档 { $message_id }
telegram-file-metadata = { $size } · 消息 { $message_id }
telegram-file-download-action = 下载
telegram-file-select-action = 选择文件
telegram-files-more-action = 加载更早的文件
telegram-batch-title = 批量下载
telegram-batch-description = 按发送时间和一种或多种文件类型筛选当前已加载页面。
telegram-batch-period-label = 发送时间
telegram-batch-period-any = 不限时间
telegram-batch-period-24h = 最近 24 小时
telegram-batch-period-7d = 最近 7 天
telegram-batch-period-30d = 最近 30 天
telegram-batch-kind-label = 文件类型
telegram-batch-kind-all = 全部文件
telegram-batch-kind-video = 视频
telegram-batch-kind-document = 文档
telegram-batch-kind-archive = 压缩包
telegram-batch-kind-audio = 音频
telegram-batch-kind-image = 图片
telegram-batch-kind-other = 其他
telegram-batch-download-action = 下载所选文件
telegram-batch-preparing = 正在准备所选文件…
telegram-batch-queued = 已将 { $count } 个文件加入一个下载任务组。
telegram-batch-no-matches = 尚未选择文件。
telegram-batch-failed = 无法创建批量下载，请检查连接后重试。
telegram-message-detail-title = 消息详情
telegram-message-detail-empty = 选择文件以查看消息详情。
telegram-message-file-name = 文件名
telegram-message-sent-at = 发送时间
telegram-message-mime-type = 媒体类型
telegram-message-caption = 消息说明
telegram-message-no-caption = 无消息说明
telegram-download-queued = 下载已排队
telegram-download-running = 正在下载
telegram-download-paused = 下载已暂停
telegram-download-completed = 下载完成
telegram-download-failed = 下载失败
telegram-download-cancelled = 下载已取消
detail-verification-size-checked = Telegram 文件大小校验通过
telegram-error-invalid-request = 请检查 API 凭据、手机号、验证码或密码后重试。
telegram-error-authorization = Telegram 授权已过期，请重新连接账号。
telegram-error-network = 无法连接 Telegram，请检查网络后重试。
telegram-error-persistence = 无法安全打开本地 Telegram 会话。
telegram-error-generic = Telegram 无法完成此操作。
detail-finished = 完成时间
detail-trace-id = 跟踪 ID
detail-queue-wait = 队列等待
detail-elapsed = 已用时间
detail-average-speed = 平均速度
detail-failure-reason = 失败原因
detail-failure-retryable = 此失败可能是暂时性的。请检查上述条件后重试。
detail-failure-user-action = 重试前需要更改设置、账号、来源或文件系统状态。
detail-failure-terminal = 此任务已停止，不会自动重试。
detail-verification-not-reached = 传输先发生失败，因此尚未进入校验阶段
detail-trace-timeline = 诊断时间线
trace-event-queued = 已排队
trace-event-started = 已开始
trace-event-paused = 已暂停
trace-event-resumed = 已继续
trace-event-completed = 已完成
trace-event-failed = 已失败
trace-event-cancelled = 已取消
native-download-error-invalid-request = 下载请求或目标位置无效。
native-download-error-not-found = 对应的 Telegram 消息或文档已不存在。
native-download-error-conflict = 另一个活动任务正在使用相同目标位置。
native-download-error-persistence = TeleArk 无法安全读取或写入本地传输状态。
native-download-error-source-missing = Telegram 来源文档已不可用。
native-download-error-source-changed = Telegram 来源在下载期间发生了变化。
native-download-error-permission-denied = macOS 拒绝访问所配置的下载位置。
native-download-error-capacity = 已达到传输队列或本地资源限制。
native-download-error-authorization = Telegram 授权已过期或不允许此次下载。
native-download-error-network = 网络连接或 Telegram 请求被中断。
native-download-error-cancelled = 下载在完成前被取消。
native-download-error-unknown = 下载因尚未分类的内部原因失败。
settings-managed-logs-label = 诊断日志
settings-managed-logs-restart-note = 下次启动 TeleArk 时，诊断日志将移至此位置。
settings-diagnostics-title = 诊断与性能跟踪
settings-diagnostics-description = TeleArk 在每日 JSON 日志中记录结构化操作耗时、传输生命周期事件和安全的失败类别。
settings-diagnostics-unavailable = 本次运行无法使用结构化诊断日志
settings-diagnostics-dropped-events = 有 { $count } 条诊断事件因有界写入队列已满而被丢弃
settings-diagnostics-open-action = 打开诊断日志
settings-diagnostics-privacy-note = 日志可能包含技术性的任务、频道和消息 ID、字节数、耗时及错误类别。日志不会包含 API Hash、登录令牌、密码、文件内容、手机号、文件名、说明文字或本地文件路径。
nav-no-channels = 未找到频道
nav-storage-channel = TeleArk
nav-storage-channel-telegram-files = Telegram 文件
nav-storage-channel-teleark-files = TeleArk 文件
storage-channel-title = TeleArk 存储
storage-channel-telegram-files = 原始文件
storage-channel-teleark-files = 文件
storage-channel-tabs-description = Telegram 原始对象与还原后的逻辑文件
storage-channel-managed-title = TeleArk 管理的文件
storage-channel-managed-runtime-note = TeleArk 会在这里识别软件包清单和分片。桌面端连接加密保管库的持有服务后才会提供自动解密和还原；当前 Alpha 不会把已锁定的软件包显示成已还原。
storage-channel-managed-empty = 还没有已认证的文件。上传第一个文件，或刷新以扫描此频道。
storage-channel-managed-name-locked = 已加密的逻辑文件
storage-channel-managed-detail-title = 文件详情
storage-channel-managed-detail-empty = 选择一个受管理文件以查看其软件包。
vault-password-placeholder = 输入密钥库密码
vault-new-password-placeholder = 再次输入新密码
vault-recovery-placeholder = 粘贴完整的恢复密钥
vault-status-not-configured = 尚未配置
vault-password-generation = 密码包裹 v{ $generation }
vault-operation-working = 正在处理…
vault-operation-succeeded = 密钥库操作已完成。
vault-create-password-label = 创建密码
vault-confirm-password-label = 确认密码
vault-create-action = 创建密钥库
vault-password-label = 密码
vault-unlock-password-action = 使用密码解锁
vault-recovery-key-label = 恢复密钥
vault-unlock-recovery-action = 使用恢复密钥解锁
vault-new-password-label = 新密码
vault-rotate-recovery-action = 更换恢复密钥
vault-recovery-save-now-title = 立即保存此恢复密钥
vault-recovery-save-now-description = TeleArk 只会在此时显示这份自包含恢复包。隐藏前请离线保存。更换当前恢复密钥不会撤销之前导出的灾难恢复包，请妥善保护或安全销毁旧副本。
vault-restore-title = 本地数据丢失后恢复
vault-restore-description = 在上方输入自包含恢复包，并在两个密码栏位中设置新的本地密码。
vault-recovery-bundle-label = 恢复包
vault-recovery-export-default-name = TeleArk 恢复包.txt
vault-restore-action = 恢复密钥库
vault-os-credential-title = OS Credential
vault-os-credential-development-note = 本功能正在开发中，目前不可选择。
vault-error-invalid-request = 请检查输入，并确保两次输入的密码一致。
vault-error-authorization = 密钥库密码或恢复密钥不正确，或者密钥库仍处于锁定状态。
vault-error-source-missing = 所选源文件或远端软件包不可用。
vault-error-permission-denied = TeleArk 没有访问该位置的权限。
vault-error-network = Telegram 未能完成密钥库操作，请重试。
vault-error-not-found = 找不到请求的密钥库或软件包。
vault-error-conflict = 已经配置了密钥库。
vault-error-capacity = 此操作超出了支持的大小或存储限制。
vault-error-cancelled = 密钥库操作已取消。
vault-error-persistence = 无法安全读取、验证或保存密钥库数据。
upload-file-picker-prompt = 选择文件
upload-no-file-selected = 尚未选择文件
upload-select-file-description = 请选择源文件；其原始路径不会上传。
storage-channel-managed-vault-locked = 解锁以验证文件并显示原始名称。无需解锁即可查看原始文件。
storage-channel-managed-runtime-ready = 经过认证的 manifest 会显示为逻辑文件。下载时先解密到临时文件，完成全文件校验后再发布到 TeleArk Downloads。
storage-channel-manifest-authenticated = 已认证
storage-channel-restore-ready = 可以还原
storage-channel-download-restored-action = 下载还原后的文件
transfer-vault-storage-channel = Telegram / TeleArk（已加密）
transfer-vault-encrypted-type = TeleArk 加密软件包
transfer-vault-parts-progress = { $completed } / { $total } 个分片
detail-vault-lifecycle = 任务持久性
detail-vault-lifecycle-memory-only = 仅保存在内存中的快照；暂不支持暂停、取消、重试和重启续传
detail-vault-controls-unavailable = 运行中 · 暂无控制操作
storage-channel-upload-action = 上传文件
storage-channel-package-id = 软件包 ID
storage-channel-logical-name = 原始文件名
storage-channel-manifest-state = 清单
storage-channel-manifest-found = 已找到清单
storage-channel-manifest-missing = 缺少清单
storage-channel-encoded-size = Telegram 编码后大小
storage-channel-restore-state = 还原状态
storage-channel-restore-owner-unavailable = 正在等待桌面端解锁加密保管库并启动恢复服务
storage-channel-related-files = 关联的 Telegram 文件
storage-channel-file-role = TeleArk 文件作用
storage-channel-role-manifest = 自描述清单
storage-channel-role-part = 加密应用分片 { $index }
storage-channel-why-file-exists = 此文件为何存在
storage-channel-manifest-explanation = 此清单描述原始逻辑文件、加密分片、完整性数据，以及恢复所需的 Telegram 消息。
storage-channel-part-explanation = 此不透明文件包含较大逻辑文件的一段加密区间，需要按清单与同一软件包的其他分片组合。
upload-target-storage-channel = TeleArk 私有频道
upload-storage-channel-security-note = 文件内容、名称和元数据会先加密，再上传到你的 TeleArk 私有频道。
detail-direction = 方向
detail-direction-upload = 上传
detail-direction-download = 下载
detail-storage-format = 存储格式
detail-storage-format-native = Telegram 原生文件（未改动）
detail-storage-format-teleark = TeleArk 加密多分片软件包
detail-content-protection = 内容保护
detail-content-protection-none = 无；Telegram 存储原始文件字节
detail-content-protection-aes = AES-256-GCM 认证加密
detail-integrity-codec = 完整性与分帧
detail-integrity-native = Telegram 声明长度；暂不提供内容哈希
detail-integrity-teleark = BLAKE3 明文/密文摘要；8 MiB 认证帧
detail-manifest-codec = 清单格式
detail-manifest-codec-value = teleark-manifest-v1（暂定）
transfer-preview-upload-note = 上传预览；桌面端尚未连接持久运行的加密上传服务。
transfer-summary-uploading = 上传中
action-load-more = 加载更多
transfer-controller-title = 自适应传输控制器
transfer-mode-live = 实时
transfer-mode-replay = 回放
transfer-replay-previous = 上一条
transfer-replay-next = 下一条
transfer-replay-position = 第 {$current} / {$total} 条决策
transfer-controller-phase = 控制器阶段
transfer-controller-parameters = C / W / F / P / E / Qe
transfer-controller-goodput = 实际有效吞吐
transfer-controller-encryption-throughput = 加密吞吐
transfer-controller-disk-throughput = 磁盘吞吐
transfer-controller-bdp = 估算 BDP
transfer-controller-rtt = RTT p95
transfer-controller-inflight = Inflight 字节
transfer-controller-inflight-value = 当前 {$current} / 目标 {$target}
transfer-controller-cpu = 加密 worker CPU 使用率
transfer-controller-bottleneck = 当前瓶颈
transfer-controller-memory = 传输内存
transfer-controller-memory-value = 已用 {$used} / 预算 {$budget}
transfer-controller-part-map = 分片状态图
transfer-controller-part-map-value = 完成 {$completed} · 传输中 {$inflight} · 重试 {$retry} · 失败 {$failed} · 缺失 {$missing}
transfer-controller-connections-value = C={$connections} · W={$rpcs}
transfer-controller-queue-waits-value = 网络等待加密 {$network} · 加密等待网络 {$encryption} · {$parts} 分片/秒
transfer-controller-buffers-value = 明文 {$plaintext} · 密文 {$encrypted} · 网络 {$network} · 写入 {$writer}
transfer-controller-decisions = 控制器决策
transfer-controller-no-decisions = 正在等待第一条测量决策。
transfer-controller-lanes = DC 与连接 lane
transfer-controller-lanes-unavailable = 当前 grammers 传输层尚未暴露可信的逐 DC lane 指标。
transfer-lane-value = DC {$dc} · lane {$lane} · inflight {$inflight} · {$speed} · RTT p95 {$rtt} · {$status}
transfer-lane-active = 活跃
transfer-lane-paused = 已暂停
transfer-session-log = 永久会话日志
transfer-phase-ramp = RAMP
transfer-phase-probe = PROBE
transfer-phase-stable = STABLE
transfer-phase-recover = RECOVER
transfer-bottleneck-unknown = 正在收集证据
transfer-bottleneck-encryption = CPU 加密受限
transfer-bottleneck-network = Telegram 或网络受限
transfer-bottleneck-disk = 磁盘受限
transfer-bottleneck-memory = 内存预算受限
transfer-controller-no-parameter = 无参数变化
transfer-parameter-connections = 传输连接数 (C)
transfer-parameter-rpcs = 每连接 inflight RPC 数 (W)
transfer-parameter-files = 活跃文件数 (F)
transfer-parameter-parts = 每文件 inflight part 数 (P)
transfer-parameter-encryption-workers = 加密 worker 数 (E)
transfer-parameter-encrypted-queue = 密文队列深度 (Qe)
transfer-decision-probe = PROBE
transfer-decision-keep = KEEP
transfer-decision-confirm = CONFIRM
transfer-decision-platform = PLATFORM
transfer-decision-rollback = ROLLBACK
transfer-decision-recover = RECOVER
transfer-decision-respect-soft-limit = 遵守软限制
transfer-decision-override-soft-limit = 自适应突破
transfer-decision-ignore-soft-limit = 忽略软限制
transfer-decision-pause-lane = 暂停 LANE
transfer-decision-resume-lane = 恢复 LANE
transfer-reason-initial-ramp = 初始吞吐爬升。
transfer-reason-bdp = 当前 inflight 字节低于 1.75× BDP 目标。
transfer-reason-improved = 探测令有效吞吐提升至少 3%。
transfer-reason-confirm = 探测令有效吞吐提升 1–3%，需要再次采样确认。
transfer-reason-platform = 探测增益低于 1%，上一参数是平台点。
transfer-reason-regressed = 有效吞吐下降，控制器已恢复上一参数。
transfer-reason-encryption-starved = 网络 lane 等待密文分片的时间过长。
transfer-reason-network-backpressure = 密文队列长期接近满载，因此降低加密并发。
transfer-reason-small-files = 小文件队列需要更多活跃文件 slot。
transfer-reason-large-file = 大文件流水线需要更多 inflight 分片。
transfer-reason-memory = 缓冲区与 inflight 字节超过传输内存预算。
transfer-reason-disk = 磁盘吞吐正在限制端到端有效吞吐。
transfer-reason-part-retry = 临时分片错误已触发重试，并降低了单文件并发。
transfer-reason-flood-wait = Telegram 要求此 lane 等待；其他 lane 仍可继续。
transfer-reason-flood-wait-expired = Telegram 强制等待已结束，lane 已恢复。
transfer-reason-soft-limit = 实测探测与 Telegram 的保守软限制冲突。
transfer-reason-all-platform = 所有可用参数均已到达实测平台或配置上限。
transfer-decision-throughput-value = {$elapsed} · {$before} → {$after}（{$change}）
transfer-part-timeline = 最近分片时间线
transfer-part-inflight = 传输中
transfer-part-completed = 已完成
transfer-part-retry = 重试
transfer-part-failed = 失败
transfer-part-event-value = 分片 {$part} · offset {$offset} · {$length} · {$state} · 第 {$attempt} 次 · {$elapsed}
settings-transfer-soft-limit-title = Telegram 软限制策略
settings-transfer-soft-limit-description = 当实测有效吞吐支持更多活跃工作时，选择自适应控制器如何处理 Telegram 的保守建议。
settings-transfer-soft-limit-respect = 遵守
settings-transfer-soft-limit-adaptive = 自适应突破
settings-transfer-soft-limit-ignore = 忽略
settings-transfer-soft-limit-note = 此策略控制建议性的活跃文件数量限制；下载速度行为请在“下载策略”中单独选择。冲突会显示在实时/回放界面并写入会话日志，Telegram 协议硬限制和 FLOOD_WAIT 始终必须遵守。

transfer-actions = 操作
transfer-show-details = 查看详情
transfer-close-details = 关闭详情
transfer-scope-visible = 当前列表
transfer-filter-count = { $label } · { $count }
transfer-delete-confirmation = 删除 { $count } 个任务？已下载的文件会保留，任务记录、日志和未完成的下载数据将被清除。
transfer-filter-title = 筛选
transfer-actions-applying = 正在处理任务操作…

settings-download-strategy-title = 下载策略
settings-download-strategy-balanced = 均衡
settings-download-strategy-max = 最大吞吐量
settings-download-strategy-description = 最大吞吐量快速探测至 64 路分片并发，容忍偶发网络错误，会占用更多带宽和内存，仍严格遵守服务器等待时间。原生下载开始或恢复时生效，加密保管库传输不受影响。

account-welcome = 你的文件，你的私人空间。

account-welcome-back = 欢迎回到 TeleArk

account-login = 登录

account-switch = 切换账户

account-restoring = 正在恢复 Telegram 会话…

account-change-method = 使用其他登录方式

account-private-note = Telegram 会话仅保存在这台 Mac 上。

account-switch-description = 切换会退出当前 Telegram 账户并暂停下载。文件与下载进度会保留。

account-switching = 正在暂停下载并退出账户…

account-switch-busy = 请等待当前加密传输完成后再切换账户。

shell-preview = 界面预览

shell-utilities = 工具

shell-local-library = 本地文件库

shell-account = 账户

shell-disk-summary = 可用 { $free } · TeleArk 已用 { $used }

vault-unlock-action = 解锁密钥库

vault-lock-action = 锁定密钥库

unlock-return-note = 解锁后将继续刚才的操作。TeleArk 处于活动状态时，密钥会保留在内存中。

unlock-use-recovery = 使用恢复密钥

unlock-recovery-saved = 已保存恢复密钥，继续

storage-nav-title = TeleArk

storage-private-label = 你的专属私有频道

storage-remote-description = 由 TeleArk 管理的私有加密文件存储。请保留此频道及其中的文件，以便恢复数据。

storage-setup-title = 为文件建立专属空间

storage-setup-description = 创建一个由你拥有的 Telegram 私有频道。TeleArk 会在这里存储并自动识别加密文件，你的 Saved Messages 仍由你自行使用。

storage-create-action = 创建私有频道

storage-discover-action = 查找已有的 TeleArk 频道

storage-loading = 正在检查私有存储…

storage-unavailable = 原来绑定的频道当前不可用。请刷新，或选择下面已有的 TeleArk 频道。应用不会自动替换原频道。

storage-setup-error = 暂时无法验证频道。请刷新重试；中断前的创建操作可能已经成功。

storage-locked-title = 文件已安全锁定

storage-guide-title = 如何使用 TeleArk 存储

storage-guide-done = 知道了

storage-guide-private-title = 1. 仅属于你的频道

storage-guide-private-body = 你拥有这个私有频道。TeleArk 不会添加成员或公开链接。保留频道简介中的标记，重装后即可重新发现它。

storage-guide-files-title = 2. 使用完整文件

storage-guide-files-body = 上传后，TeleArk 会加密文件并按需分片。解锁密钥库后将验证清单并显示原始文件名。下载会先解密、校验，再生成完整文件。

storage-guide-raw-title = 3. 查看原始文件

storage-guide-raw-body = 原始文件模式显示 Telegram 中的普通上传、加密分片与清单。仅凭文件名不能证明真实性。恢复文件所需的清单与全部分片都应保留。

storage-guide-key-title = 4. 保存恢复密钥

storage-guide-key-body = 请将恢复资料安全保存在这台 Mac 之外。Telegram 无法恢复你的加密密钥。高级恢复工具位于“设置 → 密钥库”。

storage-legacy-title = 旧版文件恢复

storage-legacy-description = 从 Saved Messages 恢复原有 TeleArk 文件。新上传的文件会使用专属私有频道。

storage-legacy-action = 从 Saved Messages 恢复…

settings-about = 关于

about-description = 为 Mac 打造的私人文件空间。

about-changelog-title = 更新记录

about-licenses = 开源许可

about-alpha = 预发布版本 · 加密格式仍待独立安全审查。

menu-application-about = 关于 TeleArk

menu-application-settings = 设置…

menu-view-transfers = 传输

menu-view-storage = TeleArk 存储

menu-file-upload = 上传加密文件…

about-changelog-v040 =
    ## 0.4.0 · 文件的新家

    ### 为 Mac 重新设计
    - 使用 GPUI Kit 0.6 与配套 gpui-pre 系列重写桌面层，统一原生字体、柔和背景、控件、原创矢量图形以及浅色与深色外观。
    - 传输与 TeleArk 存储固定在侧栏顶部，频道列表独立滚动。刷新频道不再改变当前页面或选择。
    - 新的居中账户页展示已登录用户的头像与姓名，并提供“登录”和“切换账户”。新会话支持手机、二维码、验证码及两步验证。
    - 需要密钥的地方可以直接解锁，并继续上传、下载或浏览操作。切换页面不会立即重新锁定密钥库。
    - 解锁弹窗支持 Tab 导航、回车提交、Escape 关闭，并在最小窗口下完整显示说明。

    ### 专属私有频道
    - 创建或重新发现由当前 Telegram 账户拥有的私有频道，在侧栏中显示独特的 TeleArk 入口。
    - 文件模式将经过认证的清单呈现为完整文件。原始文件模式保留普通文件、加密分片及清单的原始名称与元数据。
    - 内置引导说明频道归属、加密方式、原始对象、恢复密钥，以及保留清单和全部分片的原因。
    - 原来存放在 Saved Messages 中的文件仍可通过“设置 → 密钥库”恢复。新的加密上传使用专属私有频道。

    ### 功能各归其位
    - 保留传输多选、批次、暂停、继续、取消、重试、安全删除、文件检查，以及实时和回放诊断。高级选项和详情按需展开。
    - 设置清晰组织日常选项并折叠低频功能。本地导入、搜索、文件操作、索引、存储路径、吞吐策略及完整密钥生命周期均可访问。
    - 增加原生“关于”和“设置”菜单，以及传输、存储、搜索、刷新和上传快捷键。关于页包含完整的本次更新记录。
    - 同步更新英语、简体中文与日语。

    ### 可靠性与兼容性
    - 原生下载历史增加 Telegram 账户归属。切换账户会暂停下载并保留进度，其他账户不能继续或重试这些任务。
    - 数据库版本 9 保留旧历史。缺少账户信息的任务仅在恢复原有会话时绑定，不会绑定到任意新登录账户。
    - 专属频道发现会验证所有权、私有状态和简介标记；频道改名后仍保持绑定，多个候选频道由用户明确选择。
    - 精简贡献者文档，同时保留格式规范、安全约束与架构决策。文档整理前后均有 Git 恢复检查点。
    - 加密格式仍属临时设计，等待独立安全审查。常规测试不需要真实 Telegram 账户。

upload-choose-file = 选择文件…
upload-simple-description = TeleArk 会先加密文件内容与文件名，再上传到专属频道。大文件会自动分片，下载时还原为完整文件。
transfer-manage = 管理

action-close-details = 关闭详情
storage-scan-summary = { $count } 个文件 · 已跳过 { $rejected } 个未验证清单 · 最近 1,000 个清单

shell-refresh-channels = 刷新频道

# Navigation, batches, and local filesystem observations
shell-expand-navigation = 展开导航
shell-collapse-navigation = 收起导航
shell-free-disk-space = 磁盘可用 { $free }
shell-disk-space-unavailable = 无法获取磁盘空间
transfer-batch-progress = 已完成 { $completed } / { $total } · 失败 { $failed }
transfer-batch-created = 添加时间
transfer-expand-batch = 展开批次文件
transfer-collapse-batch = 收起批次文件
local-file-present = 本地可用
local-file-missing = 已从磁盘删除
local-file-size-changed = 本地文件已更改
local-file-unavailable = 无法访问本地文件
local-file-checking = 正在检查本地文件

local-file-status = 本地文件
transfer-download-again = 重新下载

upload-selection-summary = { $count } 个文件 · { $size }
upload-batch-limit = 一次最多选择 128 个文件，每个文件均独立保存。
upload-remove-file = 从选择中移除文件
upload-stop-after-current = 完成当前文件后停止
transfer-batch-upload-name = 批量上传 · { $count } 个文件

about-changelog-unreleased =
    ## 0.4.2 · 更清晰的文件库，更紧凑的传输列表

    - 用“本地文件”和“远端文件”替代“所有文件”。本地文件显示当前账号仍可访问的下载文件及手动导入的原文件；远端文件显示当前账号已索引的 Telegram 文件。文件类型使用独立筛选。
    - 本地文件分页读取当前磁盘信息，排除已删除或无法访问的文件，支持可取消的分页与搜索，不会触发传输。
    - 传输文件及批次行统一采用与文件库相同的 42 点紧凑高度。传输入口使用上传和下载双向箭头，批次详情与操作保持可用。
    - 修复本地文件删除后无法再次下载的问题：新任务避开历史记录占用的路径，并保留原记录。
    - 同步更新英语、简体中文、日语及 macOS 应用包版本信息。数据库结构与加密格式不变。

    ## 0.4.1 · 日常使用改进

    - 资料库文件详情现支持按来源账号下载已索引的 Telegram 文件，并说明来源不可用的原因。本地文件保留打开和定位操作。

    - 修复频道自动分页重复更新表格导致的崩溃；取消或过期请求不会在切换页面、账号后重新启动加载。
    - 本地文件库明确说明 Telegram 索引文件的来源，将“已上传”改为“已索引”，并显示原频道及账号范围内的消息 ID；移除占位分片表。
    - 新增可收起的图标导航、本地文件库直达入口和独立频道列表，主界面显示下载磁盘可用空间。
    - 修复原始文件和传输详情的滚动，详情面板与底层列表互不干扰。
    - 批次显示来源、文件名、数量、时间和真实成员列表。可一次检查多个上传文件；停止批次会在当前文件完成后跳过剩余文件。
    - 后台检查已下载文件，区分文件缺失、大小变化和暂时无法访问。缺失的普通下载可作为新任务重新下载。
    - 新增原创 TeleArk 图标及带应用图标的 macOS 应用包。
    - 数据库 v10 保留按账户隔离的本地下载记录，重启后仍可检查。加密和清单格式保持不变。

file-state-remote-indexed = 已索引

library-catalog-explanation = 此本地目录包含浏览或索引 Telegram 时发现的文件，以及手动导入的文件。索引记录不代表上传或下载记录。

file-detail-source-record = 来源记录

file-detail-indexed-source-note = 此元数据来自 Telegram 来源，不代表 TeleArk 上传或下载过该文件。以下标识共同定位原账号、频道中的消息；当前远端是否仍可访问尚未重新检查。

file-detail-local-source-note = 此文件从磁盘导入。导入仅记录元数据，不会将文件内容上传到 Telegram。

file-detail-source-account-id = 来源账号 ID

file-detail-source-chat-id = 来源频道 ID

library-action-account-required = 请登录索引此文件的 Telegram 账号后下载。
library-action-source-unavailable = 此记录缺少唯一的可下载消息，请前往来源频道查找文件。
library-action-managed-source = 请前往 TeleArk → 文件，解锁并恢复此加密文件。

library-tab-local = 本地文件
library-tab-remote = 远端文件
library-types-all = 所有类型
library-type-filter = 按文件类型筛选
library-visible-files = 已显示 { $count } 个文件
library-visible-size = 已显示 { $size }
library-local-explanation = 显示当前账号已下载及手动导入、且磁盘上仍可访问的文件。刷新可重新检查。
library-remote-explanation = 显示当前账号已索引的 Telegram 文件。浏览或索引频道可将文件添加到这里；这些记录不代表已下载到本地。
library-local-empty-title = 未找到本地文件
library-local-empty-description = 下载文件或从这台电脑导入文件。已删除或无法访问的文件不会显示；也可以清除搜索和类型筛选。
library-remote-empty-description = 浏览或索引频道以添加远端文件，或清除搜索和类型筛选。
file-detail-downloaded-source-note = 此本地副本从以下来源下载。当前大小和日期来自磁盘文件。
