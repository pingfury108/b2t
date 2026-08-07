# B2T · 把书变成有声书

上传 EPUB / TXT / PDF，自动按章节组织，用 edge-tts 合成语音，导出按章节 ZIP、带章节标记的单文件 MP3，以及 M4B 有声书。

- 后端：Rust（axum + sqlx/SQLite + rust-embed 内嵌前端产物）
- 前端：Astro（静态 SEO 页面 + `/app` 应用页）
- TTS：edge-tts CLI
- 音频：ffmpeg / ffprobe
- 授权：一码解锁一本书，前 3 章免费，无需注册

## 依赖

```bash
pipx install edge-tts   # 或 pip install edge-tts
brew install ffmpeg     # macOS；Linux 用对应包管理器
```

## 开发

```bash
# 前端（端口 4321，/api 代理到 3000）
cd frontend && pnpm install && pnpm dev

# 后端（端口 3000）
cargo run
```

## 构建与运行

`cargo build` 会通过 build.rs 自动构建前端（源码比 dist 新时才执行；`B2T_SKIP_FRONTEND_BUILD=1` 可跳过）。也可以手动分步：

```bash
cd frontend && pnpm install && pnpm build   # 产物在 frontend/dist，会被 rust-embed 内嵌
cargo build --release
./target/release/b2t                        # 访问 http://localhost:3000
./target/release/b2t --help                 # 查看全部参数
```

## Docker

```bash
docker build -t b2t .
docker run -d -p 3000:3000 -v b2t-data:/data \
  -e B2T_ADMIN_KEY=<你的key> b2t
```

镜像内含 edge-tts / ffmpeg；数据持久化在 `/data` 卷。推送 main 分支或 `v*` 标签时，GitHub Action 自动构建多架构镜像并推送到 GHCR（`ghcr.io/<owner>/b2t`）；同时推 Docker Hub 的方式见 `.github/workflows/docker.yml` 注释。

## CLI 与配置

所有参数均支持「CLI 优先、环境变量回退」，也可以在项目根目录放 `.env` 文件（启动时自动加载）：

```bash
./target/release/b2t --host 127.0.0.1 --port 8080 --admin-key <key> --data-dir /var/lib/b2t
./target/release/b2t gen-codes 100          # 子命令：批量生成授权码
```

| 参数 | 环境变量 | 默认 |
|---|---|---|
| `--host` | `B2T_HOST` | `0.0.0.0` |
| `--port` | `PORT` | `3000` |
| `--data-dir` | `B2T_DATA` | `./data` |
| `--admin-key` | `B2T_ADMIN_KEY` | 无（未设置则管理后台关闭） |
| — | `RUST_LOG` | `b2t=info` |

## 管理后台

设置 `--admin-key` 后，手动访问 `http://localhost:3000/admin`（隐藏入口，站点导航中不出现）。支持生成授权码（单个/批量，明文只显示一次）、筛选列表、删除（已绑定书籍的码不可删）、用量统计。

用户在书籍页面输入授权码即可解锁全书；同一授权码可在「找回书籍」处重新打开已绑定的书。

## 目录

```
src/
  parser/    # epub(zip+roxmltree) / txt(章节正则, GB18030 兜底) / pdf(pdf-inspector)
  tts/       # edge-tts 分段合成（2000 字一段，3 并发，3 次重试）
  audio.rs   # ffprobe 时长、ffmpeg concat、ffmetadata 章节、MP3/M4B 导出
  jobs.rs    # SQLite 任务队列 + 后台 worker
  license.rs # 授权码生成 / 核验（sha256 存储，原子绑定）
  routes.rs  # REST API
migrations/  # sqlx 迁移：改表结构时追加 NNNN_描述.sql（版本号递增，历史文件不改）
frontend/
  src/pages/ # index(SEO 落地页) / help / app(应用页)
```
