import { webview } from '@kit.ArkWeb';
import { BusinessError } from '@kit.BasicServicesKit';
import { common } from '@kit.AbilityKit';
import { fileIo as fs } from '@kit.CoreFileKit';
import { resourceManager } from '@kit.LocalizationKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

import { NativeBridge } from '../bridge/NativeBridge';

const DOMAIN: number = 0x0000;
const TAG: string = 'LiteCodeOhos';

// Product builds load from the reserved virtual origin. Only the interceptor
// below answers requests for this origin; nothing goes to the real network.
const ORIGIN: string = 'https://app.litecode.example';
const ENTRY_PATH: string = 'ohos.html';
const ENTRY_URL: string = `${ORIGIN}/${ENTRY_PATH}`;

// Development switch: point the WebView at the host Vite server instead.
// Keep false for offline/product builds; set true only while iterating on the
// web UI. The emulator reaches the host through 10.0.2.2.
const USE_DEV_SERVER: boolean = false;
const DEV_SERVER_URL: string = 'http://10.0.2.2:5173/ohos.html';

// Debug builds keep web inspection available; switch off before release.
const ENABLE_WEB_DEBUG: boolean = true;

const SAMPLE_DIR: string = 'sample-workspace';
// The OHOS shell now runs lc-core's real AgentLoop; the host mock
// (scripts/ohos/mock-llm.mjs) is the default provider and ignores Authorization.
// AgentPanel only enables the composer when providers_load reports a model whose
// provider has a key (apiKeyRef counts); the env ref below stays unset on the
// emulator and lc-ohos supplies a placeholder key for local mock traffic.
const PROVIDERS_JSON: string =
  '{"providers":[{"id":"local-stub","kind":"openai-responses","label":"Local Stub",' +
  '"baseUrl":"http://10.0.2.2:8765/v1","apiKeyRef":"env:LC_STUB_API_KEY","enabled":true}],' +
  '"models":[{"id":"stub-chat","provider":"local-stub","modelId":"stub-chat",' +
  '"thinking":["off"],"contextK":100,"compressAtK":80,"capabilities":[]}],' +
  '"defaultModel":"stub-chat","defaultThinking":"off",' +
  '"subagentModel":null,"subagentThinking":null}\n';

interface ProvidersTemplate {
  models?: Object[];
}

function hasProviderModels(text: string): boolean {
  try {
    const parsed = JSON.parse(text) as ProvidersTemplate;
    return Array.isArray(parsed.models) && parsed.models.length > 0;
  } catch (error) {
    return false;
  }
}

const MIME_TYPES: Record<string, string> = {
  'html': 'text/html',
  'htm': 'text/html',
  'js': 'text/javascript',
  'mjs': 'text/javascript',
  'cjs': 'text/javascript',
  'css': 'text/css',
  'json': 'application/json',
  'map': 'application/json',
  'webmanifest': 'application/manifest+json',
  'svg': 'image/svg+xml',
  'png': 'image/png',
  'jpg': 'image/jpeg',
  'jpeg': 'image/jpeg',
  'gif': 'image/gif',
  'webp': 'image/webp',
  'avif': 'image/avif',
  'ico': 'image/x-icon',
  'woff': 'font/woff',
  'woff2': 'font/woff2',
  'ttf': 'font/ttf',
  'otf': 'font/otf',
  'wasm': 'application/wasm',
  'txt': 'text/plain',
  'xml': 'application/xml',
  'mp3': 'audio/mpeg',
  'mp4': 'video/mp4'
};

function mimeOf(assetPath: string): string {
  const dot: number = assetPath.lastIndexOf('.');
  const ext: string = dot >= 0 ? assetPath.slice(dot + 1).toLowerCase() : '';
  return MIME_TYPES[ext] ?? 'application/octet-stream';
}

function isSafeAssetPath(assetPath: string): boolean {
  if (assetPath.length === 0 || assetPath.length > 1024) {
    return false;
  }
  if (assetPath.startsWith('/') || assetPath.includes('\\') || assetPath.indexOf('\u0000') >= 0) {
    return false;
  }
  // Reject drive letters and URL schemes.
  if (assetPath.includes(':')) {
    return false;
  }
  const segments: string[] = assetPath.split('/');
  if (segments.length === 0) {
    return false;
  }
  for (const segment of segments) {
    if (segment.length === 0 || segment === '.' || segment === '..') {
      return false;
    }
  }
  return true;
}

function parseAssetPath(url: string): string | undefined {
  let end: number = url.length;
  const query: number = url.indexOf('?');
  const hash: number = url.indexOf('#');
  if (query >= 0) {
    end = Math.min(end, query);
  }
  if (hash >= 0) {
    end = Math.min(end, hash);
  }
  const cleanUrl: string = url.slice(0, end);
  if (!cleanUrl.startsWith(`${ORIGIN}/`)) {
    return undefined;
  }
  let assetPath: string = cleanUrl.slice(ORIGIN.length + 1);
  if (assetPath.length === 0) {
    assetPath = ENTRY_PATH;
  }
  try {
    assetPath = decodeURIComponent(assetPath);
  } catch (error) {
    return undefined;
  }
  return isSafeAssetPath(assetPath) ? assetPath : undefined;
}

@Entry
@Component
struct Index {
  private controller: webview.WebviewController = new webview.WebviewController();
  private bridge: NativeBridge = new NativeBridge();

  aboutToAppear(): void {
    if (ENABLE_WEB_DEBUG) {
      try {
        webview.WebviewController.setWebDebuggingAccess(true);
      } catch (error) {
        const err = error as BusinessError;
        hilog.warn(DOMAIN, TAG, 'setWebDebuggingAccess failed: %{public}s', err.message);
      }
    }
    try {
      const configJson: string = this.prepareHost();
      this.bridge.attach(this.controller);
      this.bridge.init(configJson);
    } catch (error) {
      const err = error as BusinessError;
      hilog.error(DOMAIN, TAG, 'native host init failed: %{public}s', err.message);
    }
  }

  aboutToDisappear(): void {
    // The NAPI env cleanup hook owns disposal. Calling dispose here can free the
    // host while an async invoke or agent worker is still using it.
  }

  /** Copies the rawfile sample workspace into filesDir on first launch. */
  private prepareHost(): string {
    const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
    const filesDir: string = context.filesDir;
    const workspaceRoot: string = `${filesDir}/${SAMPLE_DIR}`;
    const providersFile: string = `${filesDir}/providers.json`;

    if (!fs.accessSync(workspaceRoot)) {
      hilog.info(DOMAIN, TAG, 'copying rawfile %{public}s to %{public}s', SAMPLE_DIR, workspaceRoot);
      this.copyRawTree(context.resourceManager, SAMPLE_DIR, workspaceRoot);
    }
    this.ensureProvidersFile(providersFile);

    hilog.info(DOMAIN, TAG, 'workspace root: %{public}s', workspaceRoot);
    return JSON.stringify({
      files_dir: filesDir,
      workspace_root: workspaceRoot,
      providers_file: providersFile
    });
  }

  /**
   * Seeds the providers template on first launch. Files created by older builds
   * have no models, which leaves the AgentPanel composer disabled; rewrite those
   * too (the in-app ProvidersPanel can produce a deliberate empty config, but a
   * legacy empty file is never intentional).
   */
  private ensureProvidersFile(providersFile: string): void {
    let needsTemplate: boolean = !fs.accessSync(providersFile);
    if (!needsTemplate) {
      try {
        needsTemplate = !hasProviderModels(fs.readTextSync(providersFile));
      } catch (error) {
        needsTemplate = true;
      }
    }
    if (needsTemplate) {
      hilog.info(DOMAIN, TAG, 'writing providers template to %{public}s', providersFile);
      this.writeText(providersFile, PROVIDERS_JSON);
    }
  }

  private copyRawTree(manager: resourceManager.ResourceManager, rawDir: string, destDir: string): void {
    fs.mkdirSync(destDir, true);
    const names: string[] = manager.getRawFileListSync(rawDir);
    for (const name of names) {
      const rawPath: string = `${rawDir}/${name}`;
      const destPath: string = `${destDir}/${name}`;
      let children: string[] = [];
      try {
        children = manager.getRawFileListSync(rawPath);
      } catch (error) {
        // getRawFileListSync throws for files; treat those as leaves.
        children = [];
      }
      if (children.length > 0) {
        this.copyRawTree(manager, rawPath, destPath);
        continue;
      }
      const bytes: Uint8Array = manager.getRawFileContentSync(rawPath);
      const file = fs.openSync(destPath, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
      try {
        fs.writeSync(file.fd, bytes.buffer);
      } finally {
        fs.closeSync(file);
      }
    }
  }

  private writeText(path: string, text: string): void {
    const file = fs.openSync(path, fs.OpenMode.READ_WRITE | fs.OpenMode.CREATE | fs.OpenMode.TRUNC);
    try {
      fs.writeSync(file.fd, text);
    } finally {
      fs.closeSync(file);
    }
  }

  private probeBridge(url: string): void {
    hilog.info(DOMAIN, TAG, 'page loaded: %{public}s', url);
    this.controller
      .runJavaScript('typeof window.__lcNative === "object" && typeof window.__lcNative.invoke === "function"')
      .then((ready: string) => {
        hilog.info(DOMAIN, TAG, 'page bridge ready: %{public}s', ready);
      })
      .catch((error: BusinessError) => {
        hilog.warn(DOMAIN, TAG, 'page bridge probe failed: %{public}s', error.message);
      });
  }

  private readRawfile(rawPath: string): ArrayBuffer | undefined {
    const hostContext = this.getUIContext().getHostContext();
    if (hostContext === undefined) {
      hilog.warn(DOMAIN, TAG, 'no host context while reading %{public}s', rawPath);
      return undefined;
    }
    try {
      const bytes: Uint8Array = hostContext.resourceManager.getRawFileContentSync(rawPath);
      const copy = new Uint8Array(bytes.byteLength);
      copy.set(bytes);
      return copy.buffer;
    } catch (error) {
      const err = error as BusinessError;
      hilog.debug(DOMAIN, TAG, 'rawfile miss %{public}s (%{public}d)', rawPath, err.code);
      return undefined;
    }
  }

  private notFound(): WebResourceResponse {
    const response = new WebResourceResponse();
    response.setResponseCode(404);
    response.setReasonMessage('Not Found');
    response.setResponseMimeType('text/plain');
    response.setResponseEncoding('utf-8');
    response.setResponseData('Not Found');
    response.setResponseIsReady(true);
    return response;
  }

  private serve(url: string): WebResourceResponse {
    const assetPath: string | undefined = parseAssetPath(url);
    if (assetPath === undefined) {
      hilog.debug(DOMAIN, TAG, 'blocked URL %{public}s', url);
      return this.notFound();
    }

    let data: ArrayBuffer | undefined = this.readRawfile(`web/${assetPath}`);
    if (data === undefined) {
      // Placeholder fallback for `build.mjs --skip-web`: rawfile/ohos.html is
      // committed, while rawfile/web/ holds generated assets and is ignored.
      data = this.readRawfile(assetPath);
    }
    if (data === undefined) {
      hilog.warn(DOMAIN, TAG, 'asset not found: %{public}s', assetPath);
      return this.notFound();
    }

    const response = new WebResourceResponse();
    response.setResponseCode(200);
    response.setReasonMessage('OK');
    response.setResponseMimeType(mimeOf(assetPath));
    response.setResponseEncoding('utf-8');
    response.setResponseData(data);
    response.setResponseIsReady(true);
    return response;
  }

  build() {
    Column() {
      if (USE_DEV_SERVER) {
        Web({ src: DEV_SERVER_URL, controller: this.controller })
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(false)
          .imageAccess(true)
          .javaScriptProxy({
            object: this.bridge,
            name: '__lcNative',
            methodList: ['invoke'],
            controller: this.controller
          })
          .width('100%')
          .height('100%')
          .onPageEnd((event) => {
            this.probeBridge(event.url);
          })
      } else {
        Web({ src: ENTRY_URL, controller: this.controller })
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(false)
          .imageAccess(true)
          .javaScriptProxy({
            object: this.bridge,
            name: '__lcNative',
            methodList: ['invoke'],
            controller: this.controller
          })
          .onInterceptRequest((event: OnInterceptRequestEvent): WebResourceResponse => {
            return this.serve(event.request.getRequestUrl());
          })
          .onPageEnd((event) => {
            this.probeBridge(event.url);
          })
          .width('100%')
          .height('100%')
      }
    }
    .width('100%')
    .height('100%')
  }
}
