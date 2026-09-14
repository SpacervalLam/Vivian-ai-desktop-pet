# PowerShell 脚本静态审计器（由 ps_audit.rs 经 -File 调用，一次性进程）
#
# 纯 AST 解析：Parser::ParseInput 只构建语法树，绝不执行被审计脚本。
# 输入（stdin）：JSON 对象 { "scripts": [base64(utf8 脚本)] }——base64 规避
# 控制台输入编码（中文 Windows 默认 GBK）对脚本文本的改写。
# 输出（stdout，UTF-8）：{ "results": [ { index, parse_errors, commands,
#   parameters, types } ] }，命令名经 Get-Command 解析别名（iex →
#   Invoke-Expression），分类与硬拒判定在 Rust 侧完成。
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
$out = @()
for ($i = 0; $i -lt @($payload.scripts).Count; $i++) {
    $script = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String([string]$payload.scripts[$i]))
    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($script, [ref]$tokens, [ref]$parseErrors)

    $commands = @{}
    $parameters = @{}
    $types = @{}
    $cmdAsts = $ast.FindAll({ param($a) $a -is [System.Management.Automation.Language.CommandAst] }, $true)
    foreach ($c in $cmdAsts) {
        $name = $c.GetCommandName()
        if ($name) { $commands[[string]$name] = $true }
        foreach ($el in $c.CommandElements) {
            if ($el -is [System.Management.Automation.Language.CommandParameterAst]) {
                $parameters[[string]$el.ParameterName] = $true
            }
        }
        # New-Object 的类型名是字符串实参（AST 不产生 TypeExpression），单独收集
        if ($name -and $name -ieq 'new-object' -and $c.CommandElements.Count -ge 2) {
            $arg = $c.CommandElements[1]
            if ($arg -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
                $types[[string]$arg.Value] = $true
            }
        }
    }
    $tAsts = $ast.FindAll({ param($a) $a -is [System.Management.Automation.Language.TypeExpressionAst] }, $true)
    foreach ($t in $tAsts) { $types[[string]$t.TypeName.FullName] = $true }

    # 别名/命令解析：Get-Command 只查会话元数据，与被审计脚本的执行无关。
    # 注意 AliasInfo 的 Name 仍是别名本身，真实命令名在 ResolvedCommand/Definition
    $resolved = foreach ($n in @($commands.Keys)) {
        $cmd = Get-Command -Name $n -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $cmd) {
            [string]$n
        } elseif ($cmd -is [System.Management.Automation.AliasInfo]) {
            $target = if ($cmd.ResolvedCommand) { [string]$cmd.ResolvedCommand.Name } else { [string]$cmd.Definition }
            if ($target) { $target } else { [string]$n }
        } else {
            [string]$cmd.Name
        }
    }

    $out += [pscustomobject]@{
        index        = $i
        parse_errors = @($parseErrors).Count
        commands     = @($resolved | Sort-Object -Unique)
        parameters   = @($parameters.Keys | Sort-Object -Unique)
        types        = @($types.Keys | Sort-Object -Unique)
    }
}
[pscustomobject]@{ results = $out } | ConvertTo-Json -Depth 4 -Compress
