import { defineConfig } from "blume";

export default defineConfig({
  title: "poly-crap",
  description:
    "Find the functions in a codebase that are both complex and poorly tested.",
  content: {
    root: "content",
  },
  github: {
    owner: "drew-simmons",
    repo: "poly-crap",
    branch: "main",
    dir: "docs",
  },
  deployment: {
    output: "static",
    site: "https://drew-simmons.github.io",
    base: "/poly-crap",
  },
  search: {
    provider: "orama",
  },
  ai: {
    llmsTxt: true,
  },
  seo: {
    sitemap: true,
    robots: true,
  },
  lastModified: { type: "git" },
});
