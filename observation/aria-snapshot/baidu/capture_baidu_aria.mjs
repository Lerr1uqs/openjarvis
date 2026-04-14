import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const runsRoot = path.join(__dirname, 'runs');
const targetUrl = 'https://www.baidu.com/';
const chromePath = process.env.OPENJARVIS_OBSERVE_CHROME_PATH ?? '/usr/bin/chromium-browser';

async function main() {
  await fs.mkdir(runsRoot, { recursive: true });

  const browser = await chromium.launch({
    headless: true,
    executablePath: chromePath,
    args: [
      '--no-sandbox',
      '--disable-dev-shm-usage',
      '--disable-gpu',
      '--lang=zh-CN',
    ],
  });

  try {
    const page = await browser.newPage({
      viewport: { width: 1440, height: 1080 },
      locale: 'zh-CN',
    });
    page.setDefaultTimeout(30_000);
    page.setDefaultNavigationTimeout(30_000);

    await page.goto(targetUrl, {
      waitUntil: 'domcontentloaded',
      timeout: 30_000,
    });
    await page.waitForLoadState('networkidle', { timeout: 5_000 }).catch(() => {});
    await page.locator('body').waitFor({ state: 'visible', timeout: 10_000 });

    const body = page.locator('body');
    const ariaSnapshot = await body.ariaSnapshot();
    const title = await page.title();
    const currentUrl = page.url();
    const capturedAt = new Date().toISOString();
    const runDir = path.join(runsRoot, `capture-${safeTimestamp(capturedAt)}`);

    await fs.mkdir(runDir, { recursive: true });

    const screenshotPath = path.join(runDir, 'baidu-homepage.png');
    const ariaPath = path.join(runDir, 'aria-snapshot.yaml');
    const metadataPath = path.join(runDir, 'page-metadata.json');

    await page.screenshot({
      path: screenshotPath,
      fullPage: true,
    });
    await fs.writeFile(ariaPath, ariaSnapshot, 'utf8');
    await fs.writeFile(
      metadataPath,
      JSON.stringify(
        {
          target_url: targetUrl,
          final_url: currentUrl,
          title,
          captured_at: capturedAt,
          run_dir: path.basename(runDir),
          screenshot: path.basename(screenshotPath),
          aria_snapshot: path.basename(ariaPath),
          chromium_executable: chromePath,
        },
        null,
        2,
      ),
      'utf8',
    );

    console.log(
      JSON.stringify(
        {
          ok: true,
          target_url: targetUrl,
          final_url: currentUrl,
          title,
          captured_at: capturedAt,
          output_dir: runDir,
          screenshot: screenshotPath,
          aria_snapshot: ariaPath,
          metadata: metadataPath,
        },
        null,
        2,
      ),
    );
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(
    JSON.stringify(
      {
        ok: false,
        message: error?.message ?? String(error),
        stack: error?.stack ?? null,
      },
      null,
      2,
    ),
  );
  process.exitCode = 1;
});

function safeTimestamp(value) {
  return value
    .replaceAll(':', '')
    .replaceAll('-', '')
    .replaceAll('.000', '')
    .replaceAll('.', '')
    .replace('T', 'T')
    .replace('Z', 'Z');
}
