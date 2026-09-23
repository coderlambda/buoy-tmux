# Project website and search discovery

The public homepage is https://coderlambda.github.io/buoy-tmux/.
Source files are in `website/`; no JavaScript runtime, dependency installation, or build is needed.
`.github/workflows/pages.yml` publishes only that directory when it changes on `main`.
The repository's Pages settings must use **GitHub Actions** as the publishing source.

## Preview and maintain

Run `python3 -m http.server 4173 --directory website --bind 127.0.0.1` and open
http://127.0.0.1:4173/. Check desktop and narrow layouts, download/source links, and
the FAQ disclosure controls after editing. Keep the visible copy, description, and
SoftwareApplication JSON-LD consistent with released functionality. The release link
uses `/releases/latest` so it does not require an edit for each release.

Keep the canonical URL, Open Graph URL, structured-data URL, and `sitemap.xml` aligned.
The existing app icon and README screenshot are reused; no external fonts, analytics,
cookies, third-party scripts, or client-side rendering are required.

## Google Search Console

Use the URL-prefix property `https://coderlambda.github.io/buoy-tmux/`.
The `google-site-verification` meta tag belongs to the project owner's Google account;
keep it in `index.html` to retain verification. This public verification token is intended
to be served to crawlers, not a private API credential.

Submit `https://coderlambda.github.io/buoy-tmux/sitemap.xml` in **Sitemaps**, and inspect
the homepage URL to request indexing. A successful submission is not a guarantee of
indexing or ranking. Use the property's indexing reports to diagnose crawl failures.
The GitHub repository itself is on `github.com`, outside this property's scope.

GitHub Pages project sites live under a path. A `robots.txt` in `website/` would be
served at `/buoy-tmux/robots.txt`, but crawlers only use the host's `/robots.txt`.
Do not add a project-local file and assume that it controls crawling. No root robots
file (HTTP 404) permits crawling by default; submit the sitemap directly in Search Console.
