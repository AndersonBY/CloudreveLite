# CloudreveLite 部署

CloudreveLite 面向小内存服务器和私有临时文件分享场景。SQLite 保存元数据，进程内缓存替代 Redis。上传文件应存入阿里云 OSS 或通用 S3，不应把持久卷当作文件存储。

CloudreveLite 只有一种运行模式：单实例主节点、SQLite、进程内缓存和实体回收队列。前端静态文件从镜像内只读目录直接提供，不再嵌入 Go 二进制或复制到数据卷。

支持范围有意保持很窄：保留管理员创建账号、用户名/密码、可选 TOTP、用户组、阿里云 OSS 与通用 S3、临时分享密码、过期时间和下载次数限制，以及浏览器图片/PDF 预览。WebDAV、永久直链、OAuth/OIDC、Passkey、邮件找回、批量 ZIP、压缩/解压、视频/音频/Office/文本预览、服务端缩略图、媒体元数据分析、离线下载、全文搜索和主从节点的路由及前端入口均已移除。

临时分享策略固定为默认 7 天、最大 30 天，并禁止永不过期；下载次数默认 10 次、最大 100 次，并禁止无限下载。分享过期或达到下载次数上限后立即失效。为了让最后一次下载已经取得的 OSS/S3 签名 URL 可以完成传输，原文件会在签名 URL 失效后进入回收流程；宽限时间取当前签名 URL TTL 和 1 小时中的较大值，清理任务默认每 5 分钟扫描一次。只有没有其他有效分享引用的文件才会被删除。

文件夹分享保留，访问者可以浏览文件列表并逐个下载。CloudreveLite 不提供文件夹或多文件 ZIP 下载。目录分享失效后，只要同一目录树的根、祖先或任一后代仍被其他有效分享引用，整棵目录会暂缓删除；最后一个重叠分享失效后再整体回收，避免部分删除破坏仍有效的分享。

自动删源依赖 SQLite 与进程内文件树锁，CloudreveLite 只支持单实例运行。不要对同一个数据库启动多个 CloudreveLite 副本。

## 启动

需要 Docker Engine 和 Docker Compose，并确保仓库的 `assets` 子模块已检出：

```sh
git submodule update --init --recursive
docker compose -f docker-compose.lite.yml build
docker compose -f docker-compose.lite.yml up -d
docker compose -f docker-compose.lite.yml logs -f cloudreve-lite
```

如果本机的 Docker Hub 镜像加速器不可用，可在构建时覆盖三个基础镜像；例如使用 Google 公共缓存：

```sh
docker compose -f docker-compose.lite.yml build \
  --build-arg GO_IMAGE=mirror.gcr.io/library/golang:1.25.5-alpine \
  --build-arg NODE_IMAGE=mirror.gcr.io/library/node:22-alpine \
  --build-arg RUNTIME_IMAGE=mirror.gcr.io/library/alpine:3.22
```

服务默认只监听宿主机 `127.0.0.1:5212`。首次启动会在命名卷中创建 SQLite 数据库、配置文件和前端静态文件。首次注册入口只用于创建第一个管理员账号；创建成功后，站点配置和注册 API 会自动关闭自助注册。

镜像默认限制为 256 MiB 内存、0.75 CPU，Go 运行时目标为 192 MiB。2026-07-11 在 Docker Desktop 的新建空实例中实测稳定内存约 75 MiB，镜像约 83.5 MB，Go 二进制约 56.4 MB，前端静态文件约 16.3 MB；生产负载仍需在目标服务器复测。可在启动前覆盖：

```sh
CLOUDREVE_LITE_PORT=5213 CLOUDREVE_LITE_GOMEMLIMIT=224MiB docker compose -f docker-compose.lite.yml up -d
```

如果实测发生 OOM，先把容器内存上限提高到 320-384 MiB，再根据空闲 RSS 和并发上传结果调整 `GOMEMLIMIT`。不要在低内存实例上启用媒体处理或离线下载。

## 首次配置

1. 通过首次注册入口创建管理员账号；这是唯一允许自助注册的账号，请使用强密码。
2. 登录管理后台，只为可信的朋友创建账号；首个账号创建后公开注册已经自动关闭。
3. 创建阿里云 OSS 或通用 S3 存储策略，填写 Bucket、地域 Endpoint、AccessKey 和 Secret；将该策略设为目标用户组的默认策略。
4. 配置 OSS Bucket CORS，使浏览器可以从站点域名执行直传请求。浏览器使用的 Endpoint 必须可从公网访问；服务端操作可优先走同地域内网 Endpoint。
5. 用小文件和大于 100 MiB 的文件分别验证上传、下载、带密码分享、7 天/10 次默认值、过期和删除流程。

OSS RAM 用户应只拥有目标 Bucket 和所需前缀的最小读写权限。不要把 AccessKey 写入 Compose 文件或提交到 Git；通过管理后台写入持久数据，并定期备份 `cloudreve_lite_data` 卷中的 SQLite、配置和密钥材料。

## 反向代理与安全

不要把 `5212` 直接暴露到公网。使用现有 Nginx、Caddy 或其他反向代理提供 HTTPS，并转发到 `127.0.0.1:5212`。代理应保留 `Host`、`X-Forwarded-For` 和 `X-Forwarded-Proto`，站点 URL 必须设置为最终 HTTPS 域名。

Compose 默认使用非 root 用户、只读根文件系统、移除 Linux capabilities，并启用 `no-new-privileges`。持久写入仅发生在 `/cloudreve/data`；删除命名卷会同时删除账号、分享记录和本地配置。

## 当前限制

- 已删除功能的部分上游源码仍保留在仓库中，以便继续通过 `go build ./...` 和接收上游数据库迁移；它们不进入 Lite 路由，非目标存储 SDK 也不进入主二进制依赖图。
- 分享失效和引用感知清理已有单元测试，但尚未针对真实阿里云 OSS/S3 完成端到端删除验证；上线前必须验证第 10 次下载、随后拒绝、第 1 小时后自动删源，以及另一个有效分享仍能保护同一文件。
- 前端子模块已经切到 `CloudreveLite-frontend`，生产构建只暴露 Lite 工作流。`package.json` 中仍保留部分不进入生产构建的上游依赖，后续可继续清理安装体积。
- 从旧镜像升级后，数据卷中遗留的 `data/statics` 不再被使用，可以在确认回滚方案后手工删除。
- 256 MiB 是部署目标而非所有负载下的保证。上线前必须在真实 OSS、文件大小和并发量下测量。
- `lite-v<版本号>` 标签触发测试、镜像构建与容器冒烟验证，成功后发布 Linux amd64 镜像包、Compose 和 SHA-256 校验和。部署前校验产物，通过 `docker load` 导入镜像，再使用 `docker compose -f docker-compose.lite.yml up -d --no-build --pull never` 启动。

## 下载有效期（2026-09-09 20:03 CST，实时核验）

Lite Compose 固定覆盖 `CR_SETTING_entity_url_default_ttl=86400` 和 `CR_SETTING_entity_url_cache_margin=86400`。签名 URL 有效期为 24 小时；实体 URL 缓存时间等于有效期减安全余量，结果不大于零时不保存缓存，因此每次获取链接重新签名。已有链接的期限不变，需要从分享页重新获取下载链接。

分享过期和次数限制仍阻止获取新链接；已签发的对象链接在自身期限内仍可访问。源文件回收宽限跟随有效期延长为 24 小时。24 小时并非无限续传保证，超过期限的新请求仍需重新授权。该配置同时适用于文件实体预览 URL。

- 目标文件 `lip_reading.7z`：SQLite 与 OSS Content-Length 均为 28,675,153,496 字节，末尾 1 MiB 公网 Range 请求返回 206，完整读取 1,048,576 字节。接收者停滞的具体原因仍待客户端证据确认，此测试不等同于全文件校验。
- 部署：仅重建 `cloudreve-lite`，镜像保持 `sha256:9d3549d53e83ab83002b4bbf2cff2e49a032f0cd529d798e51d6720dbf4a7902`；未变更数据库、Nginx 或其他服务。线上 Compose 为 `/home/ecs-user/cloudreve-lite/docker-compose.yml`。
- 验证来源：Compose 语法检查、容器环境和启动日志均确认两个覆盖值；容器 healthy、无 OOM，回环及公开 `/api/v4/site/ping` 均返回 HTTP 200、业务码 0，启动日志无 error/panic，内存约 22 MiB/256 MiB。未执行真实 24 小时传输测试。
- 回滚入口：同目录 `docker-compose.yml.before-download-ttl-20260909`，SHA-256 为 `c882cd6f0a6564a51ee565dd7db2e425bb60c969400912ef24ff7bd5411173b1`；恢复配置并检查语法后，仅重建 `cloudreve-lite`。已发出的长有效期 URL 不会因配置回滚而撤销，回滚前须保留其源文件至到期，避免较短回收宽限提前删源。SSH 访问正常。

## 上游核对（2026-09-09，实时核验）

证据来源：Git fetch 后逐项比对后端 `upstream/master`（`1c3fe4d`）相对共同基线 `20c95ad` 的三个提交，以及前端 `upstream/master`（`19da0fe`）与本地实现。

- `78994b9`：管理员创建和更新账号时校验邮箱格式及密码长度。已纳入对应实现和上游测试，`go test ./service/admin ./pkg/filemanager/manager ./pkg/setting` 通过，并随 4.18.1 部署到生产。
- `65f8c3f`：Go 1.26.5 工具链升级、远程驱动错误处理和文件系统错误消息调整。未纳入；工具链升级需要单独验证构建与依赖，未发现与本次下载故障有关的变更。
- `1c3fe4d`：WebDAV COPY/MOVE 与覆盖处理。Lite 不提供 WebDAV，不纳入。
- 前端 `0834841` 验证码刷新清空及聚焦行为已与本地文件一致；`19da0fe` 为自定义预览应用的主题参数，Lite 不支持该功能，不纳入。

## 4.18.1 发布（2026-09-09 20:19 CST，实时核验）

- 发布标签：[`lite-v4.18.1`](https://github.com/AndersonBY/CloudreveLite/releases/tag/lite-v4.18.1)，源码提交 `13dbd1985ad8ae9f64977168ddf6f2665fff1e33`。[标签构建](https://github.com/AndersonBY/CloudreveLite/actions/runs/34349425502) 的前端构建、Lite 测试、全包 Go 编译、镜像构建及容器冒烟验证全部通过。
- Linux amd64 镜像为 `cloudreve-lite:4.18.1`，镜像 ID `sha256:145ae835a7f287cfaa42c5f2d013f2b1cc7d9547c706fe4d023fe05ef89765b8`；镜像 revision 标签与发布提交一致。发布包 SHA-256：`0a68573f7810025ac5a01303ef5a0a462dfcc8983174172da36cd0e852d11fe2`，本机及服务器均校验通过。
- 线上仅替换 CloudreveLite 镜像，Compose 其他字段经解析比对保持一致；数据卷仍为 `cloudreve-lite_cloudreve_lite_data`，端口仍为 `127.0.0.1:5212`，下载有效期和缓存余量仍为 86,400 秒。容器 healthy、无 OOM、无重启；回环与公开健康 API 均返回 4.18.1，公开页面 HTTP 200，启动日志无 error/panic。
- 现有 CLI 实测：连续两次获取目标文件 URL，签名不同，签名有效期为 86,400 秒；OSS 末尾 1 MiB Range GET 返回 206，完整读取 1,048,576 字节，结束位置为 28,675,153,495。此测试不替代持续 24 小时下载验证。
- SQLite `quick_check` 通过；部署前后账号、文件、实体、分享、存储策略条数分别为 1、5、4、135、2。容器内存约 57 MiB/256 MiB，根盘可用约 24 GiB，SSH 恢复访问正常。证据来源为 GitHub Actions、发布产物、SSH、SQLite 只读检查及公开 API/OSS 请求。
- 一致性备份位于 `/home/ecs-user/cloudreve-lite/backups/lite-v4.18.1-20260909`，目录仅 root 可访问，包含 SQLite、应用配置、Compose 和校验清单。SQLite `integrity_check` 通过，备份 SHA-256 为 `8a8413c7f9e852402bef7a6a95dd9a7f650b6001e792651b7dfe50aeb97bc2d1`。
- 应用回滚：恢复上述备份中的 Compose 并检查语法，仅重建 `cloudreve-lite`；原镜像 `sha256:9d3549d53e83ab83002b4bbf2cff2e49a032f0cd529d798e51d6720dbf4a7902` 保留在主机，备份 Compose 保持 24 小时下载配置。数据库恢复须先停止目标容器并保留现库，校验备份后恢复，属主为 `10001:10001`；正常应用回滚不覆盖数据库。

## 分享文件原地更新（2026-11-04 00:38–00:59 CST，实时核验）

- 经用户授权，将分享 `93zir` 指向的 `embedded-model-algorithm-code.zip` 以覆盖上传创建新版本；文件 ID `6VLiJ`、分享 ID、公开链接、10 次下载配额和 2026-11-10 23:31:19 到期时间保持不变。更新前下载数为 0。
- 新文件为 1,743,390,570 字节，本地 ZIP `7z t` 完整校验通过，SHA-256 为 `0425d46cfc46483acb33cc2d0b99512f93549f1a8ab42ab22d4130030c17ff9d`；旧版本为 1,015,609,038 字节。文件详情显示总占用 2,758,999,608 字节，与两个版本之和一致，旧版可通过“管理版本”回滚。
- 公开分享页显示新文件大小 `1.6 GB`，未消耗下载次数。签名对象的 1 KiB Range GET 返回 HTTP 206、`application/zip`，范围为 `bytes 0-1023/1743390570`。容器保持 healthy、未重启、未 OOM，回环健康 API 返回 HTTP 200 和 4.18.1。
- 本轮只新增文件版本和更新分享所指向文件的当前版本，未删除旧版、未修改分享策略、容器、Nginx 或其他服务。证据来源为本机归档测试、CloudreveLite CLI、公开页面、OSS Range 请求、SSH 和容器健康检查。

## OmniCue CUDA 包原地更新（2026-11-10 16:20–16:55 CST，实时核验）

- 将 `OmniCue-Portable-NVIDIA-CUDA-0.1.12.zip` 覆盖上传为新版本，文件 ID `9mEIq` 保持不变，当前实体由 `JYKfG` 切换为 `wzvSm`。旧版为 6,706,788,585 字节，新版为 6,705,202,454 字节；SQLite 只读查询确认两个版本均保留，旧版可通过“管理版本”回滚。
- 分享 `2JjFl`、`bwGI7` 的原链接逐字比对保持不变，到期时间分别保持为 2026-11-17 12:35:53 和 2026-11-25 12:39:53 CST，剩余下载次数均为 10。两个匿名分享信息及文件详情 API 均确认同一文件 ID、新实体和新大小；校验未消耗分享下载次数。
- 本地 `7z t` 完整检查通过。通过所有者 API 获取新签名对象 URL，在服务器以只读流完整下载 6,705,202,454 字节并计算 SHA-256，与本地一致：`834526a6eef5cc5e35b0f516680c31cc63557a9060a184ba27acdee627a23e47`。签名 URL 未记录或持久化。
- 上传前一致性 SQLite 备份位于 `/home/ecs-user/cloudreve-lite/backups/omnicue-0.1.12-20261110-162154/`，目录权限为 0700，`integrity_check=ok`，数据库 SHA-256 为 `fa01ccddc86a6d10b9e47df73d21e53bd2caddac0d877128595d7babfe7e540c`，校验清单保存在同目录。文件回滚优先切换旧版本；整库恢复须遵循停写、保留现库和校验备份的既有流程。
- 最终容器 healthy、重启次数 0、未 OOM，启动时间保持 2026-11-05 13:25:05 UTC；回环及公开健康 API 为业务码 0、版本 4.18.1，上传后日志检查未见 error/panic。最终容器内存约 20.79 MiB/256 MiB、主机 available 948 MiB、根盘可用 13 GiB，Swap 已用约 1999/2047 MiB。未变更容器、Nginx 或共享主机上的其他服务，SSH 恢复入口正常。证据来源：本机归档及哈希检查、CloudreveLite CLI、匿名公开 API、OSS 完整 GET、SSH 和 SQLite 只读查询。
