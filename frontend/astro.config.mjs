import { defineConfig } from 'astro/config';

// 后端地址，默认本机 3000 端口；可用 B2T_BACKEND 覆盖（如后端改了 --port）
const backend = process.env.B2T_BACKEND || 'http://localhost:3000';

// 开发模式下把层次化客户端路由 rewrite 到对应页面
const appRouteRewrite = {
  name: 'app-route-rewrite',
  configureServer(server) {
    server.middlewares.use((req, _res, next) => {
      if (/^\/app\/book\/[^/]+\/chapter\/\d+/.test(req.url)) {
        req.url = '/app/chapter';
      } else if (/^\/app\/book\/[^/]+/.test(req.url)) {
        req.url = '/app/book';
      }
      next();
    });
  },
};

export default defineConfig({
  output: 'static',
  vite: {
    plugins: [appRouteRewrite],
    server: {
      proxy: {
        '/api': backend,
      },
    },
  },
});
