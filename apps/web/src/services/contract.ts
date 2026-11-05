// 契约类型门面。单一来源：crates/lc-proto/bindings/lc-proto.d.ts（codegen 生成，勿手改）。
//
// 为什么不是 paths / import 引用：codegen 产物是**环境式 type 别名**（无 export 语句，
// 见 codegen.rs 的单文件合并设计），import 语句拿不到名字；tsconfig `files` 把该
// .d.ts 纳入编译单元后，这些名字在全局可见，本文件再以同名导出给模块化 import 使用。
// 本文件不复制任何契约定义——只有重导出与按 `kind` 的窄化别名；上游契约变更
// 经 `pnpm contract:codegen` 重新生成，CI contract job 以 git diff --exit-code 防漂移。
//
// 若上游把产物改为带 export 的形式，应把 tsconfig files 引用换成 paths +
// import（并删除本文件的别名跳板）；在此之前本文件是 web 侧唯一的类型入口。

// 同名重导出需要跳板：模块内的本地声明会遮蔽全局名，直接 `export type X = X`
// 会循环引用；先取不同名的本地别名再 as 导出。
type CommandResultAlias = CommandResult
type DirEntryDtoAlias = DirEntryDto
type EntryKindAlias = EntryKind
type ErrorBodyAlias = ErrorBody
type ErrorCodeAlias = ErrorCode

export type {
  CommandResultAlias as CommandResult,
  DirEntryDtoAlias as DirEntryDto,
  EntryKindAlias as EntryKind,
  ErrorBodyAlias as ErrorBody,
  ErrorCodeAlias as ErrorCode,
}

// 窄化别名：导出名与全局名不同，直接引用环境契约类型
export type DirListingResult = Extract<CommandResult, { kind: 'dir_listing' }>
export type FileContentResult = Extract<CommandResult, { kind: 'file_content' }>
export type NodeStatusResult = Extract<CommandResult, { kind: 'node_status' }>
