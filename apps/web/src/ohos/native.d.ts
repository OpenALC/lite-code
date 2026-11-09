// 鸿蒙 ArkTS NativeBridge 注入的全局入口。
// invoke 返回 lc-ohos 产出的 JSON 字符串；失败时 reject 一个 ErrorBody 形状。
export interface LcNativeBridge {
  invoke(cmd: string, argsJson: string): Promise<string>
}

declare global {
  interface Window {
    /** ArkTS 通过 javaScriptProxy 注入；浏览器 dev 下不存在。 */
    __lcNative?: LcNativeBridge
    /** tauriShim 装载标记：bridge.shellKind() 据此返回 'ohos'。 */
    __LC_OHOS__?: boolean
    /** lc-ohos 事件推送入口：参数为 EventEnvelope 的 JSON 字符串。 */
    __lcOhosEmit?: (envelopeJson: string) => void
  }
}

export {}
