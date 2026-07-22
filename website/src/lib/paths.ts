/** Prefix absolute app paths with Next `basePath` (GitHub Pages: /grok-pi). */
export function withBase(path: string): string {
  if (!path.startsWith("/") || path.startsWith("//")) return path;
  const base = process.env.NEXT_PUBLIC_BASE_PATH || "";
  if (!base) return path;
  if (path === "/") return `${base}/`;
  return `${base}${path}`;
}
