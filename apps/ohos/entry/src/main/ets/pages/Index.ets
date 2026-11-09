import { webview } from '@kit.ArkWeb';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

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

  aboutToAppear(): void {
    if (!ENABLE_WEB_DEBUG) {
      return;
    }
    try {
      webview.WebviewController.setWebDebuggingAccess(true);
    } catch (error) {
      const err = error as BusinessError;
      hilog.warn(DOMAIN, TAG, 'setWebDebuggingAccess failed: %{public}s', err.message);
    }
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
          .width('100%')
          .height('100%')
          .onPageEnd((event) => {
            hilog.info(DOMAIN, TAG, 'dev page loaded: %{public}s', event.url);
          })
      } else {
        Web({ src: ENTRY_URL, controller: this.controller })
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .fileAccess(false)
          .imageAccess(true)
          .onInterceptRequest((event: OnInterceptRequestEvent): WebResourceResponse => {
            return this.serve(event.request.getRequestUrl());
          })
          .onPageEnd((event) => {
            hilog.info(DOMAIN, TAG, 'page loaded: %{public}s', event.url);
          })
          .width('100%')
          .height('100%')
      }
    }
    .width('100%')
    .height('100%')
  }
}
