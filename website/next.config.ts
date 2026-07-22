import type { NextConfig } from "next";

/**
 * GitHub Pages project site: https://<user>.github.io/grok-pi/
 * Set GITHUB_PAGES=true in CI so basePath/assetPrefix match the repo name.
 * Local `npm run dev` keeps basePath empty → http://localhost:3000
 */
const isGhPages = process.env.GITHUB_PAGES === "true";
const repoName =
  process.env.GITHUB_REPOSITORY?.split("/")[1] ||
  process.env.PAGES_BASE_PATH?.replace(/^\//, "") ||
  "grok-pi";
const basePath = isGhPages ? `/${repoName}` : "";

const nextConfig: NextConfig = {
  output: "export",
  basePath,
  assetPrefix: basePath ? `${basePath}/` : undefined,
  trailingSlash: true,
  images: {
    unoptimized: true,
  },
};

export default nextConfig;
