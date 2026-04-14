import fs from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import readline from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const runsRoot = path.join(__dirname, 'runs');
const stateRoot = path.join(__dirname, 'state');
const stateFile = path.join(stateRoot, 'browser-cookies.json');
const targetUrl = 'https://www.bilibili.com/';
const chromePath = process.env.OPENJARVIS_OBSERVE_CHROME_PATH ?? '/usr/bin/chromium-browser';

async function main() {
  const options = parseArgs(process.argv.slice(2));
  await fs.mkdir(runsRoot, { recursive: true });
  await fs.mkdir(stateRoot, { recursive: true });

  const browser = await chromium.launch({
    headless: options.headless,
    executablePath: chromePath,
    args: [
      '--no-sandbox',
      '--disable-dev-shm-usage',
      '--disable-gpu',
      '--lang=zh-CN',
    ],
  });

  try {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 1080 },
      locale: 'zh-CN',
    });
    const cookiesLoaded = options.loadStateOnOpen
      ? await loadCookies(context, options.allowMissingState)
      : 0;
    const page = await context.newPage();
    page.setDefaultTimeout(30_000);
    page.setDefaultNavigationTimeout(30_000);

    await page.goto(targetUrl, {
      waitUntil: 'domcontentloaded',
      timeout: 30_000,
    });
    await page.waitForLoadState('networkidle', { timeout: 8_000 }).catch(() => {});
    await page.locator('body').waitFor({ state: 'visible', timeout: 15_000 });

    if (options.mode === 'login') {
      await waitForManualConfirmation(page);
      await page.waitForLoadState('networkidle', { timeout: 5_000 }).catch(() => {});
    }

    const capturedAt = new Date().toISOString();
    const runDir = path.join(runsRoot, `capture-${safeTimestamp(capturedAt)}-${options.mode}`);
    await fs.mkdir(runDir, { recursive: true });

    const title = await page.title();
    const currentUrl = page.url();
    const ariaSnapshot = await page.locator('body').ariaSnapshot();
    const screenshotPath = path.join(runDir, 'bilibili-homepage.png');
    const ariaPath = path.join(runDir, 'aria-snapshot.yaml');
    const metadataPath = path.join(runDir, 'page-metadata.json');
    const cookiesSaved = await exportCookies(context);

    await page.screenshot({
      path: screenshotPath,
      fullPage: true,
    });
    await fs.writeFile(ariaPath, ariaSnapshot, 'utf8');
    await fs.writeFile(
      metadataPath,
      JSON.stringify(
        {
          mode: options.mode,
          headless: options.headless,
          target_url: targetUrl,
          final_url: currentUrl,
          title,
          captured_at: capturedAt,
          run_dir: path.basename(runDir),
          state_file: path.relative(runDir, stateFile),
          cookies_loaded: cookiesLoaded,
          cookies_saved: cookiesSaved,
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
          mode: options.mode,
          headless: options.headless,
          target_url: targetUrl,
          final_url: currentUrl,
          title,
          captured_at: capturedAt,
          output_dir: runDir,
          state_file: stateFile,
          cookies_loaded: cookiesLoaded,
          cookies_saved: cookiesSaved,
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

async function loadCookies(context, allowMissingState) {
  let raw;
  try {
    raw = await fs.readFile(stateFile, 'utf8');
  } catch (error) {
    if (allowMissingState && error?.code === 'ENOENT') {
      return 0;
    }
    if (error?.code === 'ENOENT') {
      throw new Error(
        `cookies state file is missing: ${stateFile}. Run login mode first or pass --allow-missing-state.`,
      );
    }
    throw error;
  }

  const parsed = JSON.parse(raw);
  const cookies = Array.isArray(parsed) ? parsed : parsed?.cookies;
  if (!Array.isArray(cookies)) {
    throw new Error(`invalid cookies state file: ${stateFile}`);
  }
  if (cookies.length === 0) {
    return 0;
  }
  await context.addCookies(cookies);
  return cookies.length;
}

async function exportCookies(context) {
  const cookies = await context.cookies();
  const normalizedCookies = cookies.map((cookie) => ({
    name: cookie.name,
    value: cookie.value,
    domain: cookie.domain,
    path: cookie.path,
    expires: cookie.expires,
    httpOnly: Boolean(cookie.httpOnly),
    secure: Boolean(cookie.secure),
    sameSite: cookie.sameSite ?? 'Lax',
  }));
  await fs.writeFile(
    stateFile,
    JSON.stringify(
      {
        version: 1,
        exported_at: new Date().toISOString(),
        cookies: normalizedCookies,
      },
      null,
      2,
    ),
    'utf8',
  );
  return normalizedCookies.length;
}

async function waitForManualConfirmation(page) {
  console.log('已打开 B 站首页，当前模式为 login。');
  console.log('请在浏览器里手动注册/登录，完成后回到终端按回车继续。');
  await page.bringToFront().catch(() => {});
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
  try {
    await rl.question('登录完成后按回车保存 cookies 并采集 ARIA snapshot: ');
  } finally {
    rl.close();
  }
}

function parseArgs(argv) {
  const mode = argv[0] && !argv[0].startsWith('--') ? argv[0] : 'capture';
  if (mode !== 'login' && mode !== 'capture') {
    throw new Error(`unsupported mode: ${mode}`);
  }

  const flags = new Set(argv.filter((value) => value.startsWith('--')));
  return {
    mode,
    headless: flags.has('--headless'),
    loadStateOnOpen: mode === 'capture' || flags.has('--reuse-state'),
    allowMissingState: mode === 'login' || flags.has('--allow-missing-state'),
  };
}

function safeTimestamp(value) {
  return value
    .replaceAll(':', '')
    .replaceAll('-', '')
    .replaceAll('.000', '')
    .replaceAll('.', '')
    .replace('T', 'T')
    .replace('Z', 'Z');
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
