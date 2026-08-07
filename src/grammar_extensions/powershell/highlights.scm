; PowerShell highlighting adapted from tree-sitter-grammars/tree-sitter-powershell.

; Keywords
;---------

[
  "param"
  "dynamicparam"
  "begin"
  "process"
  "end"
  "if"
  "elseif"
  "else"
  "switch"
  "foreach"
  "for"
  "while"
  "do"
  "until"
  "function"
  "filter"
  "workflow"
  "break"
  "continue"
  "throw"
  "return"
  "exit"
  "trap"
  "try"
  "catch"
  "finally"
  "data"
  "inlinescript"
  "parallel"
  "sequence"
] @keyword

; Operators
;----------

[
  "-as"
  "-ccontains"
  "-ceq"
  "-cge"
  "-cgt"
  "-cle"
  "-clike"
  "-clt"
  "-cmatch"
  "-cne"
  "-cnotcontains"
  "-cnotlike"
  "-cnotmatch"
  "-contains"
  "-creplace"
  "-csplit"
  "-eq"
  "-ge"
  "-gt"
  "-icontains"
  "-ieq"
  "-ige"
  "-igt"
  "-ile"
  "-ilike"
  "-ilt"
  "-imatch"
  "-in"
  "-ine"
  "-inotcontains"
  "-inotlike"
  "-inotmatch"
  "-ireplace"
  "-is"
  "-isnot"
  "-isplit"
  "-join"
  "-le"
  "-like"
  "-lt"
  "-match"
  "-ne"
  "-notcontains"
  "-notin"
  "-notlike"
  "-notmatch"
  "-replace"
  "-shl"
  "-shr"
  "-split"
  "-and"
  "-or"
  "-xor"
  "-band"
  "-bor"
  "-bxor"
  "+"
  "-"
  "/"
  "\\"
  "%"
  "*"
  ".."
  "-not"
] @operator

; Punctuation
;------------

";" @punctuation.delimiter

; Literals
;---------

(string_literal) @string

(integer_literal) @number
(real_literal) @number

; Functions and Commands
;-----------------------

(command
  command_name: (command_name) @function)

(function_statement
  (function_name) @function)

(invokation_expression
  (member_name) @function)

; Types and Properties
;---------------------

(type_spec) @type

(member_access
  (member_name) @property)

; Variables
;----------

(variable) @variable

; Comments
;---------

(comment) @comment

; Arrays
;-------

(array_expression) @array

; Assignment
;-----------

(assignment_expression
  value: (pipeline) @assignvalue)

; Command invocation operator
;----------------------------

(command_invokation_operator) @operator