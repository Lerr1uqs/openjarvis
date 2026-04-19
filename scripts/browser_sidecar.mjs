import fs from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import readline from 'node:readline';
import { chromium } from 'playwright';

const headless = readBoolEnv('OPENJARVIS_BROWSER_HEADLESS', true);
const keepArtifacts = readBoolEnv('OPENJARVIS_BROWSER_KEEP_ARTIFACTS', false);
const sessionDir = process.env.OPENJARVIS_BROWSER_SESSION_DIR ?? '';
const userDataDir = process.env.OPENJARVIS_BROWSER_USER_DATA_DIR ?? '';
const chromePathOverride = process.env.OPENJARVIS_BROWSER_CHROME_PATH ?? '';
const cookiesStateFile = process.env.OPENJARVIS_BROWSER_COOKIES_STATE_FILE ?? '';
const loadCookiesOnOpen = readBoolEnv('OPENJARVIS_BROWSER_LOAD_COOKIES_ON_OPEN', false);
const saveCookiesOnClose = readBoolEnv('OPENJARVIS_BROWSER_SAVE_COOKIES_ON_CLOSE', false);
const launchTimeoutMs = Number.parseInt(
  process.env.OPENJARVIS_BROWSER_LAUNCH_TIMEOUT_MS ?? '30000',
  10,
);
const snapshotElementDefaultLimit = Number.parseInt(
  process.env.OPENJARVIS_BROWSER_SNAPSHOT_MAX_ELEMENTS ?? '200',
  10,
);
const diagnosticsBufferLimit = normalizeBoundedLimit(
  process.env.OPENJARVIS_BROWSER_DIAGNOSTICS_BUFFER_LIMIT,
  200,
  1,
  1000,
);
const diagnosticsQueryDefaultLimit = 20;

let browserContext = null;
let page = null;
let attachedBrowser = null;
let sessionMode = null;
let refIndex = new Map();
let consoleRecords = [];
let errorRecords = [];
let requestRecords = new Map();
let configuredPages = new WeakSet();
let configuredContexts = new WeakSet();
let diagnosticWriteQueue = Promise.resolve();
let diagnosticWriteError = null;

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
    case 'open':
      return openSession(request);
    case 'navigate':
      return navigate(request.url);
    case 'console':
      return consoleDiagnostics(request);
    case 'errors':
      return errorDiagnostics(request);
    case 'requests':
      return requestDiagnosticsQuery(request);
    case 'aria_snapshot':
      return ariaSnapshot();
    case 'snapshot':
      return snapshot(request.max_elements);
    case 'click_ref':
      return clickRef(request.ref);
    case 'type_ref':
      return typeRef(request.ref, request.text, Boolean(request.submit));
    case 'screenshot':
      return screenshot(request.path);
    case 'export_cookies':
      return exportCookies(request.path);
    case 'close':
      return closeBrowser();
    default:
      throw sidecarError(
        'bad_request',
        `unsupported browser action: ${request.action ?? 'unknown'}`,
      );
  }
}

async function ensurePage() {
  await ensureSessionOpen({ mode: 'launch' });
  return ensurePageFromCurrentContext();
}

async function ensureSessionOpen(defaultRequest) {
  if (sessionMode && browserContext) {
    return;
  }

  await openSession(defaultRequest);
}

async function ensurePageFromCurrentContext() {
  if (page && !page.isClosed()) {
    return page;
  }

  if (browserContext) {
    const openPages = collectContextPages(browserContext);
    if (openPages.length > 0) {
      page = openPages[openPages.length - 1];
      configurePage(page);
      return page;
    }
    page = await browserContext.newPage();
    configurePage(page);
    return page;
  }

  throw sidecarError('no_session', 'browser session is not open');
}

async function openSession(request) {
  const openRequest = normalizeOpenRequest(request);
  await disposeCurrentSession();

  try {
    if (openRequest.mode === 'launch') {
      await openLaunchSession();
      await ensureDiagnosticArtifactFiles();
      const cookiesLoaded = await maybeAutoLoadCookies();
      const currentPage = await ensurePageFromCurrentContext();
      refIndex.clear();
      return {
        action: 'open',
        mode: sessionMode,
        url: currentPage.url(),
        title: await currentPage.title(),
        cookies_loaded: cookiesLoaded,
      };
    }

    await openAttachSession(openRequest.cdp_endpoint);
    await ensureDiagnosticArtifactFiles();
    const currentPage = await ensurePageFromCurrentContext();
    refIndex.clear();
    return {
      action: 'open',
      mode: sessionMode,
      url: currentPage.url(),
      title: await currentPage.title(),
      cookies_loaded: 0,
    };
  } catch (error) {
    await hardResetSession();
    throw error;
  }
}

async function openLaunchSession() {
  assertLaunchConfig();
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
  sessionMode = 'launch';
  configureBrowserContext(browserContext);
}

async function openAttachSession(cdpEndpoint) {
  if (!cdpEndpoint || typeof cdpEndpoint !== 'string' || !cdpEndpoint.trim()) {
    throw sidecarError('bad_request', 'attach mode requires a non-empty cdp_endpoint');
  }

  attachedBrowser = await chromium.connectOverCDP(cdpEndpoint);
  const existingPages = attachedBrowser
    .contexts()
    .flatMap((context) => collectContextPages(context));
  if (existingPages.length > 0) {
    page = existingPages[existingPages.length - 1];
    browserContext = page.context();
    configureBrowserContext(browserContext);
    configurePage(page);
    sessionMode = 'attach';
    return;
  }

  if (attachedBrowser.contexts().length > 0) {
    browserContext = attachedBrowser.contexts()[0];
    page = await browserContext.newPage();
  } else {
    browserContext = await attachedBrowser.newContext();
    page = await browserContext.newPage();
  }
  configureBrowserContext(browserContext);
  configurePage(page);
  sessionMode = 'attach';
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

async function consoleDiagnostics(request) {
  return {
    action: 'console',
    entries: selectRecentRecords(consoleRecords, request?.limit),
  };
}

async function errorDiagnostics(request) {
  return {
    action: 'errors',
    entries: selectRecentRecords(errorRecords, request?.limit),
  };
}

async function requestDiagnosticsQuery(request) {
  return {
    action: 'requests',
    entries: selectRecentRequestRecords(request?.limit, Boolean(request?.failed_only)),
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

async function ariaSnapshot() {
  const currentPage = await ensurePage();
  return {
    action: 'aria_snapshot',
    url: currentPage.url(),
    title: await currentPage.title(),
    aria_snapshot: await currentPage.locator('body').ariaSnapshot({ mode: 'ai' }),
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

async function exportCookies(filePath) {
  if (!browserContext || !sessionMode) {
    throw sidecarError('no_session', 'browser session is not open');
  }
  if (!filePath || typeof filePath !== 'string') {
    throw sidecarError('bad_request', 'export_cookies requires a non-empty path');
  }

  return exportCookiesToPath(filePath);
}

async function closeBrowser() {
  refIndex.clear();
  return disposeCurrentSession();
}

function resolveRef(reference) {
  const selector = refIndex.get(reference);
  if (!selector) {
    throw sidecarError('missing_ref', `unknown browser ref: ${reference}`);
  }
  return selector;
}

function configureBrowserContext(nextContext) {
  if (!nextContext || configuredContexts.has(nextContext)) {
    return;
  }

  configuredContexts.add(nextContext);
  for (const existingPage of collectContextPages(nextContext)) {
    configurePage(existingPage);
  }
  nextContext.on('page', (nextPage) => {
    configurePage(nextPage);
  });
  nextContext.on('request', (request) => {
    recordRequestStarted(request);
  });
  nextContext.on('response', (response) => {
    recordRequestFinished(response);
  });
  nextContext.on('requestfailed', (request) => {
    recordRequestFailed(request);
  });
}

function configurePage(nextPage) {
  if (!nextPage || configuredPages.has(nextPage)) {
    return;
  }

  configuredPages.add(nextPage);
  nextPage.setDefaultTimeout(launchTimeoutMs);
  nextPage.setDefaultNavigationTimeout(launchTimeoutMs);
  nextPage.on('console', (message) => {
    recordConsoleMessage(nextPage, message);
  });
  nextPage.on('pageerror', (error) => {
    recordPageError(nextPage, error);
  });
}

function normalizeSnapshotLimit(maxElements) {
  const candidate = Number.parseInt(String(maxElements ?? snapshotElementDefaultLimit), 10);
  if (!Number.isFinite(candidate)) {
    return snapshotElementDefaultLimit;
  }
  return Math.min(Math.max(candidate, 1), 500);
}

function normalizeDiagnosticLimit(limit) {
  const candidate = Number.parseInt(String(limit ?? diagnosticsQueryDefaultLimit), 10);
  if (!Number.isFinite(candidate)) {
    return diagnosticsQueryDefaultLimit;
  }
  return Math.min(Math.max(candidate, 1), diagnosticsBufferLimit);
}

function normalizeBoundedLimit(rawValue, fallback, min, max) {
  const candidate = Number.parseInt(String(rawValue ?? fallback), 10);
  if (!Number.isFinite(candidate)) {
    return fallback;
  }
  return Math.min(Math.max(candidate, min), max);
}

function selectRecentRecords(records, limit) {
  const resolvedLimit = normalizeDiagnosticLimit(limit);
  return records.slice(-resolvedLimit).reverse();
}

function selectRecentRequestRecords(limit, failedOnly) {
  const resolvedLimit = normalizeDiagnosticLimit(limit);
  const filtered = Array.from(requestRecords.values())
    .map((entry) => entry.record)
    .filter((record) => !failedOnly || isFailedRequestRecord(record));
  return filtered.slice(-resolvedLimit).reverse();
}

function isFailedRequestRecord(record) {
  return record.result === 'http_error' || record.result === 'failed';
}

function recordConsoleMessage(nextPage, message) {
  const location = typeof message.location === 'function' ? message.location() : null;
  const record = {
    timestamp: new Date().toISOString(),
    level: normalizeConsoleLevel(message.type?.() ?? 'log'),
    text: message.text?.() ?? '',
    page_url: nextPage.url(),
    location: normalizeConsoleLocation(location),
  };
  pushConsoleRecord(record);
}

function recordPageError(nextPage, error) {
  const record = {
    timestamp: new Date().toISOString(),
    kind: 'page_error',
    message: error?.message ?? String(error),
    page_url: nextPage.url(),
    request_url: null,
    reason: null,
  };
  pushErrorRecord(record);
}

function recordRequestStarted(request) {
  const record = {
    timestamp: new Date().toISOString(),
    method: request.method(),
    url: request.url(),
    resource_type: request.resourceType(),
    status: null,
    result: 'pending',
  };
  requestRecords.set(request, {
    record,
    finalWritten: false,
  });
  trimRequestRecords();
}

function recordRequestFinished(response) {
  const request = response.request();
  const entry = requestRecords.get(request) ?? createDetachedRequestEntry(request);
  entry.record.status = response.status();
  entry.record.result = response.ok() ? 'ok' : 'http_error';
  queueRequestArtifactWrite(entry);
}

function recordRequestFailed(request) {
  const entry = requestRecords.get(request) ?? createDetachedRequestEntry(request);
  const failureReason = request.failure()?.errorText ?? null;
  entry.record.result = 'failed';
  const record = {
    timestamp: new Date().toISOString(),
    kind: 'request_failed',
    message: failureReason ?? 'request failed',
    page_url: safePageUrlFromRequest(request),
    request_url: request.url(),
    reason: failureReason,
  };
  pushErrorRecord(record);
  queueRequestArtifactWrite(entry);
}

function createDetachedRequestEntry(request) {
  const entry = {
    record: {
      timestamp: new Date().toISOString(),
      method: request.method(),
      url: request.url(),
      resource_type: request.resourceType(),
      status: null,
      result: 'pending',
    },
    finalWritten: false,
  };
  requestRecords.set(request, entry);
  trimRequestRecords();
  return entry;
}

function trimRequestRecords() {
  while (requestRecords.size > diagnosticsBufferLimit) {
    const oldestRequest = requestRecords.keys().next().value;
    const oldestEntry = requestRecords.get(oldestRequest);
    if (oldestEntry) {
      queueRequestArtifactWrite(oldestEntry);
    }
    requestRecords.delete(oldestRequest);
  }
}

function pushConsoleRecord(record) {
  consoleRecords.push(record);
  trimRecordBuffer(consoleRecords);
  queueDiagnosticArtifactWrite('console.jsonl', record);
}

function pushErrorRecord(record) {
  errorRecords.push(record);
  trimRecordBuffer(errorRecords);
  queueDiagnosticArtifactWrite('errors.jsonl', record);
}

function trimRecordBuffer(records) {
  while (records.length > diagnosticsBufferLimit) {
    records.shift();
  }
}

function queueRequestArtifactWrite(entry) {
  if (!keepArtifacts || entry.finalWritten) {
    return;
  }

  entry.finalWritten = true;
  queueDiagnosticArtifactWrite('requests.jsonl', entry.record);
}

function normalizeConsoleLevel(level) {
  switch (String(level ?? 'log').toLowerCase()) {
    case 'info':
      return 'info';
    case 'warning':
    case 'warn':
      return 'warn';
    case 'error':
      return 'error';
    case 'debug':
      return 'debug';
    case 'log':
    default:
      return 'log';
  }
}

function normalizeConsoleLocation(location) {
  if (!location || typeof location !== 'object' || !location.url) {
    return null;
  }

  return {
    url: location.url,
    line_number: Number.isFinite(location.lineNumber) ? location.lineNumber : 0,
    column_number: Number.isFinite(location.columnNumber) ? location.columnNumber : 0,
  };
}

function safePageUrlFromRequest(request) {
  try {
    const frame = typeof request.frame === 'function' ? request.frame() : null;
    const ownerPage = frame && typeof frame.page === 'function' ? frame.page() : null;
    return ownerPage ? ownerPage.url() : null;
  } catch {
    return null;
  }
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

function collectContextPages(context) {
  return context.pages().filter((candidate) => !candidate.isClosed());
}

function normalizeOpenRequest(request) {
  const mode = request?.mode ?? 'launch';
  if (mode !== 'launch' && mode !== 'attach') {
    throw sidecarError('bad_request', `unsupported browser open mode: ${mode}`);
  }
  if (mode === 'attach' && (!request?.cdp_endpoint || !String(request.cdp_endpoint).trim())) {
    throw sidecarError('bad_request', 'attach mode requires a non-empty cdp_endpoint');
  }
  if (mode === 'launch' && request?.cdp_endpoint) {
    throw sidecarError('bad_request', 'launch mode does not accept cdp_endpoint');
  }

  return {
    mode,
    cdp_endpoint: request?.cdp_endpoint ?? null,
  };
}

function assertLaunchConfig() {
  if (!sessionDir || !userDataDir) {
    throw sidecarError(
      'invalid_config',
      'OPENJARVIS_BROWSER_SESSION_DIR and OPENJARVIS_BROWSER_USER_DATA_DIR are required',
    );
  }
}

async function maybeAutoLoadCookies() {
  if (sessionMode !== 'launch' || !loadCookiesOnOpen || !cookiesStateFile) {
    return 0;
  }

  try {
    return await loadCookiesFromPath(cookiesStateFile, true);
  } catch (error) {
    throw sidecarError(
      error.code ?? 'cookies_load_failed',
      `failed to auto-load cookies from ${cookiesStateFile}: ${error.message ?? error}`,
    );
  }
}

async function loadCookiesFromPath(filePath, allowMissing) {
  let raw;
  try {
    raw = await fs.readFile(filePath, 'utf8');
  } catch (error) {
    if (allowMissing && error?.code === 'ENOENT') {
      return 0;
    }
    throw sidecarError(
      error?.code === 'ENOENT' ? 'cookies_state_missing' : 'cookies_state_read_failed',
      `failed to read cookies state file ${filePath}: ${error.message ?? error}`,
    );
  }

  const cookies = parseCookiesState(raw, filePath);
  if (cookies.length === 0) {
    return 0;
  }
  await browserContext.addCookies(cookies);
  return cookies.length;
}

async function exportCookiesToPath(filePath) {
  const cookies = await browserContext.cookies();
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
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(
    filePath,
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

  return {
    action: 'export_cookies',
    mode: sessionMode,
    path: filePath,
    cookie_count: normalizedCookies.length,
  };
}

function parseCookiesState(raw, filePath) {
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw sidecarError(
      'cookies_state_parse_failed',
      `failed to parse cookies state file ${filePath}: ${error.message ?? error}`,
    );
  }

  const cookies = Array.isArray(parsed) ? parsed : parsed?.cookies;
  if (!Array.isArray(cookies)) {
    throw sidecarError(
      'cookies_state_invalid',
      `cookies state file ${filePath} must contain a top-level cookies array`,
    );
  }

  return cookies.map((cookie, index) => normalizeCookie(cookie, filePath, index));
}

function normalizeCookie(cookie, filePath, index) {
  if (!cookie || typeof cookie !== 'object') {
    throw sidecarError(
      'cookies_state_invalid',
      `cookie #${index + 1} in ${filePath} must be an object`,
    );
  }
  for (const field of ['name', 'value', 'domain', 'path']) {
    if (typeof cookie[field] !== 'string') {
      throw sidecarError(
        'cookies_state_invalid',
        `cookie #${index + 1} in ${filePath} is missing string field ${field}`,
      );
    }
  }

  return {
    name: cookie.name,
    value: cookie.value,
    domain: cookie.domain,
    path: cookie.path,
    expires: Number.isFinite(cookie.expires) ? cookie.expires : -1,
    httpOnly: Boolean(cookie.httpOnly),
    secure: Boolean(cookie.secure),
    sameSite: normalizeSameSite(cookie.sameSite),
  };
}

function normalizeSameSite(value) {
  const normalized = String(value ?? 'Lax');
  switch (normalized.toLowerCase()) {
    case 'strict':
      return 'Strict';
    case 'none':
      return 'None';
    case 'lax':
    default:
      return 'Lax';
  }
}

async function ensureDiagnosticArtifactFiles() {
  if (!keepArtifacts || !sessionDir) {
    return;
  }

  await fs.mkdir(sessionDir, { recursive: true });
  await Promise.all([
    fs.appendFile(path.join(sessionDir, 'console.jsonl'), '', 'utf8'),
    fs.appendFile(path.join(sessionDir, 'errors.jsonl'), '', 'utf8'),
    fs.appendFile(path.join(sessionDir, 'requests.jsonl'), '', 'utf8'),
  ]);
}

function queueDiagnosticArtifactWrite(fileName, record) {
  if (!keepArtifacts || !sessionDir) {
    return;
  }

  const payload = `${JSON.stringify(record)}\n`;
  diagnosticWriteQueue = diagnosticWriteQueue
    .catch(() => {})
    .then(async () => {
      try {
        await fs.appendFile(path.join(sessionDir, fileName), payload, 'utf8');
      } catch (error) {
        diagnosticWriteError ??= error;
        throw error;
      }
    });
}

async function flushPendingRequestArtifacts() {
  for (const entry of requestRecords.values()) {
    queueRequestArtifactWrite(entry);
  }
}

async function flushDiagnosticArtifactWrites() {
  try {
    await diagnosticWriteQueue;
  } catch {
    // The first write failure is reported below.
  }
  if (diagnosticWriteError) {
    throw sidecarError(
      'diagnostic_artifact_write_failed',
      `failed to write browser diagnostic artifacts: ${diagnosticWriteError.message ?? diagnosticWriteError}`,
    );
  }
}

async function disposeCurrentSession() {
  const closeResult = {
    action: 'close',
    closed: Boolean(sessionMode),
    mode: sessionMode,
    exported_cookies_path: null,
    exported_cookie_count: null,
  };

  if (!sessionMode) {
    return closeResult;
  }

  await flushPendingRequestArtifacts();
  await flushDiagnosticArtifactWrites();

  if (sessionMode === 'launch' && saveCookiesOnClose && cookiesStateFile && browserContext) {
    const exportResult = await exportCookiesToPath(cookiesStateFile);
    closeResult.exported_cookies_path = exportResult.path;
    closeResult.exported_cookie_count = exportResult.cookie_count;
  }

  if (sessionMode === 'launch' && browserContext) {
    await browserContext.close().catch(() => {});
  }
  if (sessionMode === 'attach' && attachedBrowser) {
    await attachedBrowser.close().catch(() => {});
  }

  resetSessionState();
  return closeResult;
}

async function hardResetSession() {
  try {
    await flushPendingRequestArtifacts().catch(() => {});
    await flushDiagnosticArtifactWrites().catch(() => {});
    if (browserContext && sessionMode === 'launch') {
      await browserContext.close().catch(() => {});
    }
    if (attachedBrowser && sessionMode === 'attach') {
      await attachedBrowser.close().catch(() => {});
    }
  } finally {
    resetSessionState();
  }
}

function resetSessionState() {
  browserContext = null;
  attachedBrowser = null;
  page = null;
  sessionMode = null;
  refIndex.clear();
  consoleRecords = [];
  errorRecords = [];
  requestRecords = new Map();
  configuredPages = new WeakSet();
  configuredContexts = new WeakSet();
  diagnosticWriteQueue = Promise.resolve();
  diagnosticWriteError = null;
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
