import { webview } from '@kit.ArkWeb';
import { createHandle } from 'libentry.so';
import type { CoreHandle } from 'libentry.so';

@Entry
@Component
struct Index {
  controller: webview.WebviewController = new webview.WebviewController();
  @State handleText: string = '';

  aboutToAppear(): void {
    const handle: CoreHandle = createHandle();
    this.handleText = `NAPI handle: ${handle.id} (native=${handle.native})`;
  }

  build() {
    Column({ space: 4 }) {
      Text(this.handleText)
        .fontSize(12)
        .fontColor('#f0a020')
        .padding(4)
      Web({ src: 'http://10.0.2.2:5173/', controller: this.controller })
        .javaScriptAccess(true)
        .domStorageAccess(true)
        .fileAccess(true)
        .imageAccess(true)
        .width('100%')
        .layoutWeight(1)
    }
    .width('100%')
    .height('100%')
  }
}
