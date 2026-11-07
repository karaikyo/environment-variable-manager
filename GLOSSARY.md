# Windows Environment Manager

Vocabulary for managing environment variables and their recorded changes.

## Language

**User variable**:
An environment variable saved for the current Windows user.
_Avoid_: Personal variable

**System variable**:
An environment variable saved at the local computer scope.
_Avoid_: Global variable, which can be confused with a variable inside a program

**App-made edit**:
A change applied through this environment-variable manager.

**External edit**:
A change made outside this environment-variable manager, such as through another tool or the Windows environment-variable interface.

**Variable history**:
The recorded changes associated with a variable within its user or system scope on a particular computer.

**Observed change**:
A difference between two observed states of a saved user or system variable. It records what the app observed, without promising every intervening edit, its exact time, or its author.
_Avoid_: Complete audit trail

**Value entry**:
One semicolon-delimited segment of an environment variable's stored value. Order, whitespace, duplicates, and empty segments are significant; each entry occupies its own line in the interface.
