// Function catalog for autocomplete and argument hints. IronCalc's own function
// table isn't exported, so this lists the common Excel functions with their
// signatures; the engine still decides what a formula means.

export interface FnInfo { name: string; args: string[]; desc: string; category: string }

const F = (category: string, list: [string, string, string][]): FnInfo[] =>
  list.map(([name, sig, desc]) => ({ name, args: sig ? sig.split(", ") : [], desc, category }));

export const FUNCTIONS: FnInfo[] = [
  ...F("Math", [
    ["SUM", "number1, [number2], …", "Adds numbers"],
    ["SUMIF", "range, criteria, [sum_range]", "Adds cells that meet a condition"],
    ["SUMIFS", "sum_range, criteria_range1, criteria1, …", "Adds cells that meet several conditions"],
    ["SUMPRODUCT", "array1, [array2], …", "Multiplies arrays and adds the products"],
    ["PRODUCT", "number1, [number2], …", "Multiplies numbers"],
    ["ROUND", "number, num_digits", "Rounds to a number of digits"],
    ["ROUNDUP", "number, num_digits", "Rounds away from zero"],
    ["ROUNDDOWN", "number, num_digits", "Rounds toward zero"],
    ["INT", "number", "Rounds down to an integer"],
    ["TRUNC", "number, [num_digits]", "Cuts off the decimal part"],
    ["MOD", "number, divisor", "Remainder after division"],
    ["ABS", "number", "Absolute value"],
    ["POWER", "number, power", "Raises to a power"],
    ["SQRT", "number", "Square root"],
    ["EXP", "number", "e raised to a power"],
    ["LN", "number", "Natural logarithm"],
    ["LOG", "number, [base]", "Logarithm"],
    ["LOG10", "number", "Base-10 logarithm"],
    ["PI", "", "The number π"],
    ["RAND", "", "Random number between 0 and 1"],
    ["RANDBETWEEN", "bottom, top", "Random integer in a range"],
    ["CEILING", "number, significance", "Rounds up to a multiple"],
    ["FLOOR", "number, significance", "Rounds down to a multiple"],
    ["MROUND", "number, multiple", "Rounds to a multiple"],
    ["SIGN", "number", "Sign of a number"],
    ["SUBTOTAL", "function_num, ref1, …", "Subtotal of a list"],
    ["SEQUENCE", "rows, [columns], [start], [step]", "List of sequential numbers"],
  ]),
  ...F("Statistics", [
    ["AVERAGE", "number1, [number2], …", "Average of numbers"],
    ["AVERAGEIF", "range, criteria, [average_range]", "Average of cells that meet a condition"],
    ["AVERAGEIFS", "average_range, criteria_range1, criteria1, …", "Average with several conditions"],
    ["COUNT", "value1, [value2], …", "Counts numbers"],
    ["COUNTA", "value1, [value2], …", "Counts non-empty cells"],
    ["COUNTBLANK", "range", "Counts empty cells"],
    ["COUNTIF", "range, criteria", "Counts cells that meet a condition"],
    ["COUNTIFS", "criteria_range1, criteria1, …", "Counts with several conditions"],
    ["MAX", "number1, [number2], …", "Largest value"],
    ["MIN", "number1, [number2], …", "Smallest value"],
    ["MAXIFS", "max_range, criteria_range1, criteria1, …", "Largest value with conditions"],
    ["MINIFS", "min_range, criteria_range1, criteria1, …", "Smallest value with conditions"],
    ["MEDIAN", "number1, [number2], …", "Middle value"],
    ["MODE", "number1, [number2], …", "Most common value"],
    ["LARGE", "array, k", "k-th largest value"],
    ["SMALL", "array, k", "k-th smallest value"],
    ["RANK", "number, ref, [order]", "Rank of a number in a list"],
    ["STDEV", "number1, [number2], …", "Standard deviation (sample)"],
    ["STDEV.S", "number1, [number2], …", "Standard deviation (sample)"],
    ["STDEV.P", "number1, [number2], …", "Standard deviation (population)"],
    ["VAR.S", "number1, [number2], …", "Variance (sample)"],
    ["PERCENTILE", "array, k", "k-th percentile"],
    ["QUARTILE", "array, quart", "Quartile of a data set"],
  ]),
  ...F("Logical", [
    ["IF", "logical_test, value_if_true, [value_if_false]", "Returns one value if true, another if false"],
    ["IFS", "logical_test1, value_if_true1, …", "First value whose test is true"],
    ["IFERROR", "value, value_if_error", "A fallback when a formula errors"],
    ["IFNA", "value, value_if_na", "A fallback for #N/A"],
    ["AND", "logical1, [logical2], …", "True if all are true"],
    ["OR", "logical1, [logical2], …", "True if any is true"],
    ["NOT", "logical", "Reverses true and false"],
    ["XOR", "logical1, [logical2], …", "Exclusive or"],
    ["SWITCH", "expression, value1, result1, …", "Picks a result by matching a value"],
    ["TRUE", "", "The value TRUE"],
    ["FALSE", "", "The value FALSE"],
  ]),
  ...F("Lookup", [
    ["XLOOKUP", "lookup_value, lookup_array, return_array, [if_not_found], [match_mode], [search_mode]", "Finds a value and returns the matching item"],
    ["VLOOKUP", "lookup_value, table_array, col_index_num, [range_lookup]", "Looks down the first column of a table"],
    ["HLOOKUP", "lookup_value, table_array, row_index_num, [range_lookup]", "Looks across the first row of a table"],
    ["INDEX", "array, row_num, [column_num]", "Value at a position in a range"],
    ["MATCH", "lookup_value, lookup_array, [match_type]", "Position of a value in a range"],
    ["XMATCH", "lookup_value, lookup_array, [match_mode], [search_mode]", "Position of a value"],
    ["CHOOSE", "index_num, value1, [value2], …", "Picks from a list by position"],
    ["OFFSET", "reference, rows, cols, [height], [width]", "Reference offset from a cell"],
    ["INDIRECT", "ref_text, [a1]", "Reference from text"],
    ["ROW", "[reference]", "Row number"],
    ["COLUMN", "[reference]", "Column number"],
    ["ROWS", "array", "Number of rows"],
    ["COLUMNS", "array", "Number of columns"],
    ["FILTER", "array, include, [if_empty]", "Rows that meet a condition"],
    ["SORT", "array, [sort_index], [sort_order], [by_col]", "Sorted copy of a range"],
    ["UNIQUE", "array, [by_col], [exactly_once]", "Distinct values"],
    ["TRANSPOSE", "array", "Swaps rows and columns"],
  ]),
  ...F("Text", [
    ["CONCAT", "text1, [text2], …", "Joins text"],
    ["CONCATENATE", "text1, [text2], …", "Joins text"],
    ["TEXTJOIN", "delimiter, ignore_empty, text1, …", "Joins text with a delimiter"],
    ["LEFT", "text, [num_chars]", "First characters"],
    ["RIGHT", "text, [num_chars]", "Last characters"],
    ["MID", "text, start_num, num_chars", "Characters from the middle"],
    ["LEN", "text", "Number of characters"],
    ["UPPER", "text", "Uppercase"],
    ["LOWER", "text", "Lowercase"],
    ["PROPER", "text", "Capitalizes each word"],
    ["TRIM", "text", "Removes extra spaces"],
    ["SUBSTITUTE", "text, old_text, new_text, [instance_num]", "Replaces text"],
    ["REPLACE", "old_text, start_num, num_chars, new_text", "Replaces part of text"],
    ["FIND", "find_text, within_text, [start_num]", "Position of text (case-sensitive)"],
    ["SEARCH", "find_text, within_text, [start_num]", "Position of text"],
    ["TEXT", "value, format_text", "Formats a number as text"],
    ["VALUE", "text", "Converts text to a number"],
    ["REPT", "text, number_times", "Repeats text"],
    ["EXACT", "text1, text2", "True if two texts match exactly"],
    ["TEXTBEFORE", "text, delimiter, [instance_num]", "Text before a delimiter"],
    ["TEXTAFTER", "text, delimiter, [instance_num]", "Text after a delimiter"],
  ]),
  ...F("Date & time", [
    ["TODAY", "", "Today's date"],
    ["NOW", "", "Current date and time"],
    ["DATE", "year, month, day", "Builds a date"],
    ["TIME", "hour, minute, second", "Builds a time"],
    ["YEAR", "serial_number", "Year of a date"],
    ["MONTH", "serial_number", "Month of a date"],
    ["DAY", "serial_number", "Day of a date"],
    ["HOUR", "serial_number", "Hour of a time"],
    ["MINUTE", "serial_number", "Minute of a time"],
    ["WEEKDAY", "serial_number, [return_type]", "Day of the week"],
    ["WEEKNUM", "serial_number, [return_type]", "Week of the year"],
    ["EDATE", "start_date, months", "Date months before or after"],
    ["EOMONTH", "start_date, months", "Last day of a month"],
    ["DATEDIF", "start_date, end_date, unit", "Difference between dates"],
    ["NETWORKDAYS", "start_date, end_date, [holidays]", "Working days between dates"],
    ["WORKDAY", "start_date, days, [holidays]", "Date after a number of working days"],
    ["DAYS", "end_date, start_date", "Days between dates"],
  ]),
  ...F("Financial", [
    ["PMT", "rate, nper, pv, [fv], [type]", "Loan payment"],
    ["FV", "rate, nper, pmt, [pv], [type]", "Future value"],
    ["PV", "rate, nper, pmt, [fv], [type]", "Present value"],
    ["NPV", "rate, value1, [value2], …", "Net present value"],
    ["IRR", "values, [guess]", "Internal rate of return"],
    ["RATE", "nper, pmt, pv, [fv], [type], [guess]", "Interest rate per period"],
    ["NPER", "rate, pmt, pv, [fv], [type]", "Number of periods"],
  ]),
  ...F("Information", [
    ["ISBLANK", "value", "True if empty"],
    ["ISNUMBER", "value", "True if a number"],
    ["ISTEXT", "value", "True if text"],
    ["ISERROR", "value", "True if any error"],
    ["ISNA", "value", "True if #N/A"],
    ["NA", "", "The #N/A error"],
  ]),
];

export const FN_BY_NAME = new Map(FUNCTIONS.map((f) => [f.name, f]));

/** The function name being typed at the caret, if any (e.g. "VLO" in "=SUM(VLO"). */
export function tokenAtCaret(text: string, caret: number): { start: number; prefix: string } | null {
  if (!text.startsWith("=")) return null;
  const before = text.slice(0, caret);
  const m = /([A-Za-z][A-Za-z0-9.]*)$/.exec(before);
  if (!m) return null;
  const start = caret - m[1].length;
  const prev = before[start - 1];
  // not inside a string, and not a cell reference like A1
  if ((before.slice(0, start).match(/"/g)?.length ?? 0) % 2 === 1) return null;
  if (/^[A-Za-z]{1,3}\d+$/.test(m[1]) || (prev && /[A-Za-z0-9_$!.]/.test(prev))) return null;
  return { start, prefix: m[1] };
}

export function suggest(prefix: string): FnInfo[] {
  const p = prefix.toUpperCase();
  return FUNCTIONS.filter((f) => f.name.startsWith(p)).sort((a, b) => a.name.length - b.name.length || a.name.localeCompare(b.name)).slice(0, 8);
}

/** The innermost function call around the caret and which argument it's on. */
export function callAtCaret(text: string, caret: number): { fn: FnInfo; arg: number } | null {
  if (!text.startsWith("=")) return null;
  const stack: { name: string; arg: number }[] = [];
  let inStr = false;
  const s = text.slice(0, caret);
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (ch === '"') inStr = !inStr;
    if (inStr) continue;
    if (ch === "(") {
      const m = /([A-Za-z][A-Za-z0-9.]*)$/.exec(s.slice(0, i));
      stack.push({ name: m ? m[1].toUpperCase() : "", arg: 0 });
    } else if (ch === ")") stack.pop();
    else if (ch === "," && stack.length) stack[stack.length - 1].arg++;
  }
  for (let i = stack.length - 1; i >= 0; i--) {
    const fn = FN_BY_NAME.get(stack[i].name);
    if (fn) return { fn, arg: stack[i].arg };
  }
  return null;
}

/** Cell references in a formula, for colored highlights while editing. */
export function formulaRefs(text: string): { r0: number; c0: number; r1: number; c1: number }[] {
  if (!text.startsWith("=")) return [];
  const out: { r0: number; c0: number; r1: number; c1: number }[] = [];
  const col = (s: string) => [...s.toUpperCase()].reduce((n, ch) => n * 26 + ch.charCodeAt(0) - 64, 0);
  // skip strings and refs to other sheets (Sheet1!A1)
  const clean = text.replace(/"[^"]*"/g, (m) => " ".repeat(m.length));
  const re = /(^|[^A-Za-z0-9_!.$'])\$?([A-Za-z]{1,3})\$?(\d+)(?::\$?([A-Za-z]{1,3})\$?(\d+))?(?![A-Za-z0-9_(!])/g;
  for (let m; (m = re.exec(clean)); ) {
    const c0 = col(m[2]), r0 = +m[3];
    const c1 = m[4] ? col(m[4]) : c0, r1 = m[5] ? +m[5] : r0;
    out.push({ r0: Math.min(r0, r1), c0: Math.min(c0, c1), r1: Math.max(r0, r1), c1: Math.max(c0, c1) });
  }
  return out;
}

export const REF_COLORS = ["#2f6fdb", "#d6336c", "#2b8a3e", "#e8590c", "#7048e8", "#0c8599", "#c2255c", "#5c940d"];

/** Change the number of decimals shown by a number format. */
export function adjustDecimals(fmt: string, delta: number): string {
  if (!fmt || fmt.toLowerCase() === "general") return delta > 0 ? "0.0" : "0";
  return fmt.split(";").map((section) => {
    if (/[dmyhs]/i.test(section.replace(/"[^"]*"|\[[^\]]*\]/g, ""))) return section; // dates/times: leave alone
    const at = section.indexOf("0");
    if (at < 0) return section;
    const lastZero = section.lastIndexOf("0");
    const dot = section.indexOf(".", at);
    const n = dot >= 0 && dot < lastZero ? lastZero - dot : 0;
    const next = Math.max(0, n + delta);
    const intEnd = dot >= 0 && dot < lastZero ? dot : lastZero + 1;
    const after = section.slice(lastZero + 1);
    return section.slice(0, intEnd) + (next ? "." + "0".repeat(next) : "") + after;
  }).join(";");
}
