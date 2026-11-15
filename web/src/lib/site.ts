/**
 * The one source of the site's identity. The canonical URL, the sitemap, the
 * Open Graph tags, the JSON-LD, robots.txt and llms.txt all read it from here.
 */
export const SITE = {
  // The root of a host, never a path, or crawlers never find robots.txt and
  // llms.txt. `wrangler.jsonc` routes the same host.
  url: 'https://snob.dennisgr7.dev',
  name: 'snob',
  /** What people search for before they know the name. */
  category: 'Instagram client for the desktop',
  /** What snob is in one line: the manifests' and the README's, word for word. */
  description:
    'Native Instagram client for the desktop, with a command line, an MCP server for AI agents, automations and webhooks',
  repository: 'https://github.com/dennisgr7/snob',
  /** The repository's files as they are on `main`, where the install scripts are read from. */
  raw: 'https://raw.githubusercontent.com/dennisgr7/snob/main',
  license: 'https://github.com/dennisgr7/snob/blob/main/LICENSE',
  locale: 'en',
  ogLocale: 'en_US',
  /** The footer signature, the one mention of who made it. */
  author: { name: 'Dennis', url: 'https://dennisgr7.dev' },
  /** The browser bar, per theme: the page background of each. */
  themeColor: { light: '#fafafa', dark: '#0b0d10' },
} as const;
