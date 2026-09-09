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

- `78994b9`：管理员创建和更新账号时校验邮箱格式及密码长度。本地已纳入对应实现和上游测试，`go test ./service/admin ./pkg/filemanager/manager ./pkg/setting` 通过；生产镜像尚未包含此补丁。
- `65f8c3f`：Go 1.26.5 工具链升级、远程驱动错误处理和文件系统错误消息调整。未纳入；工具链升级需要单独验证构建与依赖，未发现与本次下载故障有关的变更。
- `1c3fe4d`：WebDAV COPY/MOVE 与覆盖处理。Lite 不提供 WebDAV，不纳入。
- 前端 `0834841` 验证码刷新清空及聚焦行为已与本地文件一致；`19da0fe` 为自定义预览应用的主题参数，Lite 不支持该功能，不纳入。
