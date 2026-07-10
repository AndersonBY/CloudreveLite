# CloudreveLite 部署

CloudreveLite 面向小内存服务器和私有临时文件分享场景。SQLite 保存元数据，进程内缓存替代 Redis。上传文件应存入阿里云 OSS 或通用 S3，不应把持久卷当作文件存储。

当前阶段已实现独立的 `server --lite` 启动模式、磁盘静态文件、精简后台队列，以及本文的非 root 单容器部署面。后端仍会编译部分上游代码，前端也仍是完整上游构建。

最终支持范围有意保持很窄：保留管理员创建账号、用户名/密码、可选 TOTP、用户组、阿里云 OSS 与通用 S3、临时分享密码、过期时间和下载次数限制，以及浏览器原生图片/PDF 预览。公开注册、OAuth/OIDC、Passkey、邮件找回、WebDAV、批量 ZIP、压缩/解压、视频/音频/Office 预览、服务端缩略图、媒体元数据分析、离线下载、全文搜索和主从节点属于后续裁剪目标；当前阶段不能把这些全部视为已经从二进制或前端移除。

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

镜像默认限制为 256 MiB 内存、0.75 CPU，Go 运行时目标为 192 MiB。2026-07-11 在 Docker Desktop 的新建空实例中实测空闲内存约 38 MiB，最终镜像约 128 MiB；生产负载仍需在目标服务器复测。可在启动前覆盖：

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

- 第一阶段仍使用上游完整路由和前端。最终移除项是已经确认的裁剪边界，目前不能把隐藏入口当作安全边界。
- 分享失效和引用感知清理已有单元测试，但尚未针对真实阿里云 OSS/S3 完成端到端删除验证；上线前必须验证第 10 次下载、随后拒绝、第 1 小时后自动删源，以及另一个有效分享仍能保护同一文件。
- 前端子模块已经切到 `CloudreveLite-frontend`，并收紧临时分享表单与分享目录下载入口；大部分上游页面和依赖仍待后续裁剪。
- 镜像升级不会覆盖已持久化的 `data/statics`。前后端版本不匹配时，停止服务、备份后删除该目录，再启动容器让 entrypoint 重新生成。
- 256 MiB 是部署目标而非所有负载下的保证。上线前必须在真实 OSS、文件大小和并发量下测量。
- 当前工作流只执行测试和镜像构建，不发布镜像。
