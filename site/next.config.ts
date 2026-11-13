import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Cloudflare Pages serves the generated HTML, JavaScript, and assets directly.
  output: "export",
  images: { unoptimized: true },
};

export default nextConfig;
