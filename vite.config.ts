import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [
    react(),
    {
      name: "filem-local-origin",
      configureServer(server) {
        server.middlewares.use((req, res, next) => {
          if (req.url?.startsWith("/api")) {
            const origin = req.headers.origin;
            if (
              (origin &&
                !["http://127.0.0.1:5178", "http://localhost:5178"].includes(
                  origin,
                )) ||
              req.headers["sec-fetch-site"] === "cross-site"
            ) {
              res.statusCode = 403;
              res.end("Local origin required");
              return;
            }
          }
          next();
        });
      },
    },
  ],
  server: {
    host: "127.0.0.1",
    port: 5178,
    strictPort: true,
    cors: false,
    proxy: process.env.FILEM_DEV_TOKEN
      ? {
          "/api": {
            target: "http://127.0.0.1:4318",
            headers: { authorization: `Bearer ${process.env.FILEM_DEV_TOKEN}` },
          },
        }
      : undefined,
    watch: { ignored: ["**/.tools/**", "**/target/**", "**/.filem/**"] },
  },
  clearScreen: false,
});
