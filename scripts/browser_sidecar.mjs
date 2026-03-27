import fs from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import readline from 'node:readline';
import { chromium } from 'playwright';

const headless = readBoolEnv('OPENJARVIS_BROWSER_HEADLESS', true);
const sessionDir = process.env.OPENJARVIS_BROWSER_SESSION_DIR ?? '';
const userDataDir = process.env.OPENJARVIS_BROWSER_USER_DATA_DIR ?? '';
const chromePathOverride = process.env.OPENJARVIS_BROWSER_CHROME_PATH ?? '';
const launchTimeoutMs = Number.parseInt(
  process.env.OPENJARVIS_BROWSER_LAUNCH_TIMEOUT_MS ?? '30000',
  10,
);
const snapshotElementDefaultLimit = Number.parseInt(
  process.env.OPENJARVIS_BROWSER_SNAPSHOT_MAX_ELEMENTS ?? '200',
  10,
);

let browserContext = null;
let page = null;
let refIndex = new Map();

process.on('SIGINT', async () => {
  await closeBrowser();
  process.exit(0);
});

process.on('SIGTERM', async () => {
  await closeBrowser();
  process.exit(0);
});

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

for await (const line of rl) {
  const trimmed = line.trim();
  if (!trimmed) {
    continue;
  }

  let request = null;
  try {
    request = JSON.parse(trimmed);
    const result = await handleRequest(request);
    process.stdout.write(`${JSON.stringify({
      id: request.id,
      ok: true,
      result,
    })}\n`);
    if (request.action === 'close') {
      break;
    }
  } catch (error) {
    const sidecarError = toSidecarError(error);
    process.stdout.write(`${JSON.stringify({
      id: request?.id ?? 'unknown',
      ok: false,
      error: sidecarError,
    })}\n`);
  }
}

async function handleRequest(request) {
  switch (request.action) {
    case 'navigate':
      return navigate(request.url);
    case 'snapshot':
      return snapshot(request.max_elements);
    case 'click_ref':
      return clickRef(request.ref);
    case 'type_ref':
      return typeRef(request.ref, request.text, Boolean(request.submit));
    case 'screenshot':
      return screenshot(request.path);
    case 'close':
      await closeBrowser();
      return {
        action: 'close',
        closed: true,
      };
    default:
      throw sidecarError(
        'bad_request',
        `unsupported browser action: ${request.action ?? 'unknown'}`,
      );
  }
}

async function ensurePage() {
  if (page && !page.isClosed()) {
    return page;
  }

  if (browserContext) {
    const openPages = browserContext.pages().filter((candidate) => !candidate.isClosed());
    if (openPages.length > 0) {
      page = openPages[openPages.length - 1];
      configurePage(page);
      return page;
    }
  }

  if (!sessionDir || !userDataDir) {
    throw sidecarError(
      'invalid_config',
      'OPENJARVIS_BROWSER_SESSION_DIR and OPENJARVIS_BROWSER_USER_DATA_DIR are required',
    );
  }

  await fs.mkdir(sessionDir, { recursive: true });
  await fs.mkdir(userDataDir, { recursive: true });

  const executablePath = await resolveChromeExecutable();
  browserContext = await chromium.launchPersistentContext(userDataDir, {
    headless,
    executablePath,
    viewport: {
      width: 1440,
      height: 960,
    },
    args: [
      '--no-first-run',
      '--no-default-browser-check',
    ],
  });

  const existingPages = browserContext.pages();
  page = existingPages.length > 0 ? existingPages[0] : await browserContext.newPage();
  configurePage(page);
  return page;
}

async function navigate(url) {
  if (!url || typeof url !== 'string') {
    throw sidecarError('bad_request', 'navigate requires a non-empty url');
  }

  const currentPage = await ensurePage();
  await currentPage.goto(url, {
    waitUntil: 'domcontentloaded',
    timeout: launchTimeoutMs,
  });
  await currentPage.waitForLoadState('networkidle', { timeout: 1000 }).catch(() => {});
  refIndex.clear();

  return {
    action: 'navigate',
    url: currentPage.url(),
    title: await currentPage.title(),
  };
}

async function snapshot(maxElements) {
  const currentPage = await ensurePage();
  const elementLimit = normalizeSnapshotLimit(maxElements);
  const observed = await currentPage.evaluate((limit) => {
    const interactiveRoles = new Set([
      'button',
      'link',
      'textbox',
      'searchbox',
      'combobox',
      'menuitem',
      'option',
      'tab',
      'checkbox',
      'radio',
      'switch',
    ]);

    const isVisible = (element) => {
      const style = window.getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return style.visibility !== 'hidden'
        && style.display !== 'none'
        && rect.width > 0
        && rect.height > 0
        && style.opacity !== '0';
    };

    const buildSelector = (element) => {
      if (element.id) {
        return `#${element.id}`;
      }

      const segments = [];
      let current = element;
      while (current && current.nodeType === Node.ELEMENT_NODE && current.tagName.toLowerCase() !== 'html') {
        const tagName = current.tagName.toLowerCase();
        let index = 1;
        let sibling = current.previousElementSibling;
        while (sibling) {
          if (sibling.tagName === current.tagName) {
            index += 1;
          }
          sibling = sibling.previousElementSibling;
        }
        segments.unshift(`${tagName}:nth-of-type(${index})`);
        current = current.parentElement;
      }
      return segments.join(' > ');
    };

    const inferRole = (element) => {
      const tagName = element.tagName.toLowerCase();
      if (tagName === 'a') {
        return 'link';
      }
      if (tagName === 'input') {
        const inputType = (element.getAttribute('type') ?? '').toLowerCase();
        if (inputType === 'checkbox') {
          return 'checkbox';
        }
        if (inputType === 'radio') {
          return 'radio';
        }
        return 'textbox';
      }
      if (tagName === 'textarea') {
        return 'textbox';
      }
      if (tagName === 'select') {
        return 'combobox';
      }
      if (tagName === 'button') {
        return 'button';
      }
      return element.getAttribute('role') ?? tagName;
    };

    const describeText = (value) => value.replace(/\s+/g, ' ').trim();
    const inferSectionHint = (element) => {
      let current = element;
      while (current && current !== document.body) {
        const tagName = current.tagName?.toLowerCase?.() ?? '';
        if (['main', 'article', 'section', 'header', 'nav', 'footer', 'aside', 'form'].includes(tagName)) {
          return tagName;
        }
        current = current.parentElement;
      }
      return 'body';
    };
    const sectionOrder = (sectionHint) => {
      switch (sectionHint) {
        case 'main':
        case 'article':
        case 'form':
          return 0;
        case 'section':
          return 1;
        case 'body':
          return 2;
        case 'aside':
          return 3;
        case 'header':
        case 'nav':
          return 10;
        case 'footer':
          return 20;
        default:
          return 5;
      }
    };
    const isInteractiveCandidate = (element) => {
      const tagName = element.tagName.toLowerCase();
      const role = (element.getAttribute('role') ?? '').toLowerCase();
      const style = window.getComputedStyle(element);
      const hasHref = tagName === 'a' && Boolean(element.getAttribute('href'));
      const hasClickHandler = typeof element.onclick === 'function' || element.hasAttribute('onclick');
      const hasTabStop = typeof element.tabIndex === 'number' && element.tabIndex >= 0;
      const hasExpandedState = element.hasAttribute('aria-expanded') || element.hasAttribute('aria-haspopup');
      return hasHref
        || tagName === 'button'
        || tagName === 'input'
        || tagName === 'textarea'
        || tagName === 'select'
        || tagName === 'summary'
        || element.isContentEditable
        || interactiveRoles.has(role)
        || hasClickHandler
        || hasTabStop
        || hasExpandedState
        || style.cursor === 'pointer';
    };

    const candidates = Array.from(document.body?.querySelectorAll('*') ?? [])
      .filter((element) => isVisible(element))
      .filter((element) => isInteractiveCandidate(element))
      .map((element) => {
        const rect = element.getBoundingClientRect();
        const sectionHint = inferSectionHint(element);
        const text = describeText(
          element.innerText
          || element.value
          || element.textContent
          || '',
        ).slice(0, 160);
        const label = describeText(
          element.getAttribute('aria-label')
          || element.getAttribute('placeholder')
          || element.getAttribute('title')
          || text,
        ).slice(0, 160);
        return {
          element,
          rectTop: rect.top,
          rectLeft: rect.left,
          rectArea: Math.round(rect.width * rect.height),
          sectionHint,
          tagName: element.tagName.toLowerCase(),
          role: inferRole(element),
          text,
          label,
        };
      })
      .sort((left, right) => {
        const sectionDelta = sectionOrder(left.sectionHint) - sectionOrder(right.sectionHint);
        if (sectionDelta !== 0) {
          return sectionDelta;
        }
        const topDelta = left.rectTop - right.rectTop;
        if (topDelta !== 0) {
          return topDelta;
        }
        const leftDelta = left.rectLeft - right.rectLeft;
        if (leftDelta !== 0) {
          return leftDelta;
        }
        return right.rectArea - left.rectArea;
      });

    const totalCandidateCount = candidates.length;
    const elements = candidates.slice(0, limit).map((candidate, index) => {
      const element = candidate.element;
      const text = describeText(
        element.innerText
        || element.value
        || element.textContent
        || '',
      ).slice(0, 160);
      const label = describeText(
        element.getAttribute('aria-label')
        || element.getAttribute('placeholder')
        || element.getAttribute('title')
        || text,
      ).slice(0, 160);
      return {
        ref: String(index + 1),
        tag_name: candidate.tagName,
        role: candidate.role,
        label,
        text,
        selector: buildSelector(element),
        href: element.getAttribute('href'),
        target: element.getAttribute('target'),
        input_type: element.getAttribute('type'),
        placeholder: element.getAttribute('placeholder'),
        section_hint: candidate.sectionHint,
        disabled: element.hasAttribute('disabled'),
      };
    });

    const bodyText = describeText(document.body?.innerText ?? '')
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .slice(0, 12)
      .join('\n');

    return {
      url: window.location.href,
      title: document.title ?? '',
      bodyText,
      elements,
      totalCandidateCount,
      truncated: totalCandidateCount > limit,
    };
  }, elementLimit);

  refIndex = new Map();
  for (const element of observed.elements) {
    refIndex.set(element.ref, element.selector);
  }

  const snapshotLines = [
    `URL: ${observed.url}`,
    `Title: ${observed.title || 'Untitled'}`,
  ];
  if (observed.bodyText) {
    snapshotLines.push(observed.bodyText);
  }
  for (const element of observed.elements) {
    const suffix = element.href ? ` -> ${element.href}` : '';
    snapshotLines.push(
      `[${element.ref}] ${element.role} ${element.label || element.text || element.tag_name}${suffix}`.trim(),
    );
  }

  return {
    action: 'snapshot',
    url: observed.url,
    title: observed.title,
    snapshot_text: snapshotLines.join('\n'),
    elements: observed.elements,
    total_candidate_count: observed.totalCandidateCount,
    truncated: observed.truncated,
  };
}

async function clickRef(reference) {
  const currentPage = await ensurePage();
  const selector = resolveRef(reference);
  const locator = currentPage.locator(selector).first();
  const beforePages = browserContext.pages().filter((candidate) => !candidate.isClosed());
  let popup = null;
  try {
    [popup] = await Promise.all([
      currentPage.waitForEvent('popup', { timeout: 1500 }).catch(() => null),
      locator.click({ timeout: launchTimeoutMs }),
    ]);
  } catch (error) {
    const href = await locator.getAttribute('href').catch(() => null);
    if (!href) {
      throw error;
    }
    await currentPage.goto(new URL(href, currentPage.url()).toString(), {
      waitUntil: 'domcontentloaded',
      timeout: launchTimeoutMs,
    });
  }

  let activePage = currentPage;
  let openedNewPage = false;
  if (popup && !popup.isClosed()) {
    activePage = popup;
    openedNewPage = true;
  } else {
    const afterPages = browserContext.pages().filter((candidate) => !candidate.isClosed());
    if (afterPages.length > beforePages.length) {
      activePage = afterPages[afterPages.length - 1];
      openedNewPage = activePage !== currentPage;
    }
  }
  await activePage.waitForLoadState('domcontentloaded', { timeout: 1500 }).catch(() => {});
  await activePage.waitForLoadState('networkidle', { timeout: 1000 }).catch(() => {});
  page = activePage;
  configurePage(page);
  refIndex.clear();

  return {
    action: 'click_ref',
    ref: reference,
    url: page.url(),
    title: await page.title(),
    opened_new_page: openedNewPage,
  };
}

async function typeRef(reference, text, submit) {
  const currentPage = await ensurePage();
  const selector = resolveRef(reference);
  const locator = currentPage.locator(selector).first();
  await locator.fill(text ?? '', {
    timeout: launchTimeoutMs,
  });
  let activePage = currentPage;
  let openedNewPage = false;
  if (submit) {
    const beforePages = browserContext.pages().filter((candidate) => !candidate.isClosed());
    const [popup] = await Promise.all([
      currentPage.waitForEvent('popup', { timeout: 1500 }).catch(() => null),
      locator.press('Enter', { timeout: launchTimeoutMs }),
    ]);
    if (popup && !popup.isClosed()) {
      activePage = popup;
      openedNewPage = true;
    } else {
      const afterPages = browserContext.pages().filter((candidate) => !candidate.isClosed());
      if (afterPages.length > beforePages.length) {
        activePage = afterPages[afterPages.length - 1];
        openedNewPage = activePage !== currentPage;
      }
    }
    await activePage.waitForLoadState('domcontentloaded', { timeout: 1500 }).catch(() => {});
    await activePage.waitForLoadState('networkidle', { timeout: 1000 }).catch(() => {});
    page = activePage;
    configurePage(page);
    refIndex.clear();
  }

  return {
    action: 'type_ref',
    ref: reference,
    url: activePage.url(),
    title: await activePage.title(),
    text_length: Array.from(text ?? '').length,
    submitted: submit,
    opened_new_page: openedNewPage,
  };
}

async function screenshot(filePath) {
  if (!filePath || typeof filePath !== 'string') {
    throw sidecarError('bad_request', 'screenshot requires a non-empty path');
  }

  const currentPage = await ensurePage();
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await currentPage.screenshot({
    path: filePath,
    fullPage: true,
  });

  return {
    action: 'screenshot',
    url: currentPage.url(),
    title: await currentPage.title(),
    path: filePath,
  };
}

async function closeBrowser() {
  refIndex.clear();
  if (browserContext) {
    await browserContext.close().catch(() => {});
  }
  browserContext = null;
  page = null;
}

function resolveRef(reference) {
  const selector = refIndex.get(reference);
  if (!selector) {
    throw sidecarError('missing_ref', `unknown browser ref: ${reference}`);
  }
  return selector;
}

function configurePage(nextPage) {
  nextPage.setDefaultTimeout(launchTimeoutMs);
  nextPage.setDefaultNavigationTimeout(launchTimeoutMs);
}

function normalizeSnapshotLimit(maxElements) {
  const candidate = Number.parseInt(String(maxElements ?? snapshotElementDefaultLimit), 10);
  if (!Number.isFinite(candidate)) {
    return snapshotElementDefaultLimit;
  }
  return Math.min(Math.max(candidate, 1), 500);
}

async function resolveChromeExecutable() {
  if (chromePathOverride) {
    await assertExecutableExists(chromePathOverride);
    return chromePathOverride;
  }

  const candidates = chromeCandidates();
  for (const candidate of candidates) {
    try {
      await assertExecutableExists(candidate);
      return candidate;
    } catch {
      // Keep probing the remaining candidates.
    }
  }

  throw sidecarError(
    'missing_chrome',
    'failed to locate a local Chrome executable; set OPENJARVIS_BROWSER_CHROME_PATH explicitly',
  );
}

function chromeCandidates() {
  const candidates = [];
  if (process.platform === 'win32') {
    const localAppData = process.env.LOCALAPPDATA ?? '';
    candidates.push(
      'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
      'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
    );
    if (localAppData) {
      candidates.push(
        path.join(localAppData, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      );
    }
    return candidates;
  }

  if (process.platform === 'darwin') {
    return [
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      path.join(process.env.HOME ?? '', 'Applications', 'Google Chrome.app', 'Contents', 'MacOS', 'Google Chrome'),
    ];
  }

  return [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/snap/bin/chromium',
  ];
}

async function assertExecutableExists(candidate) {
  await fs.access(candidate);
}

function readBoolEnv(name, defaultValue) {
  const raw = process.env[name];
  if (raw === undefined) {
    return defaultValue;
  }
  return raw === '1' || raw.toLowerCase() === 'true';
}

function sidecarError(code, message, details = null) {
  return {
    code,
    message,
    details,
  };
}

function toSidecarError(error) {
  if (error && typeof error === 'object' && 'code' in error && 'message' in error) {
    return {
      code: error.code,
      message: error.message,
      details: error.details ?? null,
    };
  }

  return {
    code: 'sidecar_error',
    message: error instanceof Error ? error.message : String(error),
    details: null,
  };
}
