import { webview } from '@kit.ArkWeb';
import { BusinessError } from '@kit.BasicServicesKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import native from 'libentry.so';

const DOMAIN: number = 0x0000;
const TAG: string = 'LiteCodeOhos';

/**
 * ArkTS half of the native bridge. The WebView exposes only `invoke` through
 * javaScriptProxy (`window.__lcNative.invoke`), matching
 * apps/web/src/ohos/tauriShim.ts. Native events are forwarded into the page via
 * `window.__lcOhosEmit(envelopeJson)`.
 */
export class NativeBridge {
  private controller: webview.WebviewController | null = null;
  private sinkRegistered: boolean = false;
  private initialized: boolean = false;
  private sheetOpen: boolean = false;

  attach(controller: webview.WebviewController): void {
    this.controller = controller;
    if (this.sinkRegistered) {
      return;
    }
    try {
      native.setEventSink((envelopeJson: string): void => {
        this.emit(envelopeJson);
      });
      this.sinkRegistered = true;
    } catch (error) {
      const err = error as BusinessError;
      hilog.error(DOMAIN, TAG, 'setEventSink failed: %{public}s', err.message);
    }
  }

  init(configJson: string): void {
    if (this.initialized) {
      this.dispose();
    }
    native.init(configJson);
    this.initialized = true;
    hilog.info(DOMAIN, TAG, 'native host initialized');
  }

  /** 页面开合供应商 sheet 时同步通知，系统返回键据此决定是否消费。 */
  setSheetOpen(open: boolean): void {
    this.sheetOpen = open;
  }

  isSheetOpen(): boolean {
    return this.sheetOpen;
  }

  /** 系统返回键关闭 sheet；runJavaScript 异步执行，先复位避免连按重复消费。 */
  closeSheet(): void {
    this.sheetOpen = false;
    const controller = this.controller;
    if (controller === null) {
      return;
    }
    controller.runJavaScript('window.__lcOhosCloseSheet && window.__lcOhosCloseSheet()')
      .catch((error: BusinessError) => {
        hilog.warn(DOMAIN, TAG, 'close sheet from back failed: %{public}s', error.message);
      });
  }

  invoke(cmd: string, argsJson: string): Promise<string> {
    if (!this.initialized) {
      return Promise.reject(new Error(JSON.stringify({
        code: 'internal',
        message: 'native host is not initialized',
        detail: null
      })));
    }
    return native.invoke(cmd, argsJson);
  }

  dispose(): void {
    if (!this.initialized) {
      return;
    }
    try {
      native.dispose();
    } catch (error) {
      const err = error as BusinessError;
      hilog.warn(DOMAIN, TAG, 'native dispose failed: %{public}s', err.message);
    }
    this.initialized = false;
  }

  private emit(envelopeJson: string): void {
    const controller = this.controller;
    if (controller === null) {
      return;
    }
    const literal: string = JSON.stringify(envelopeJson);
    controller.runJavaScript(`window.__lcOhosEmit && window.__lcOhosEmit(${literal})`)
      .catch((error: BusinessError) => {
        hilog.warn(DOMAIN, TAG, 'emit to page failed: %{public}s', error.message);
      });
  }
}
