// NAPI surface exposed by apps/ohos/entry/src/main/cpp/napi_init.cpp.
export const getBridgeVersion: () => string;
/** Creates the lc-ohos host; throws when initialization fails. Returns 1. */
export const init: (configJson: string) => number;
/** Dispatches one command; resolves to contract JSON, rejects with ErrorBody JSON. */
export const invoke: (cmd: string, argsJson: string) => Promise<string>;
/** Registers the event callback invoked on the ArkTS UI thread with envelope JSON. */
export const setEventSink: (callback: (envelopeJson: string) => void) => void;
/** Drops the host; the NAPI env cleanup hook also calls it. */
export const dispose: () => void;
