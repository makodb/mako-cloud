/** Integer minor units and their presentation. */

/** Minor-unit digits for currencies that are not two-decimal. */
const MINOR_UNIT_DIGITS: Readonly<Record<string, number>> = {
  BHD: 3,
  BIF: 0,
  CLP: 0,
  DJF: 0,
  GNF: 0,
  IQD: 3,
  ISK: 0,
  JOD: 3,
  JPY: 0,
  KMF: 0,
  KRW: 0,
  KWD: 3,
  LYD: 3,
  OMR: 3,
  PYG: 0,
  RWF: 0,
  TND: 3,
  UGX: 0,
  UYI: 0,
  VND: 0,
  VUV: 0,
  XAF: 0,
  XOF: 0,
  XPF: 0,
};

export function minorUnitDigits(currency: string): number {
  return MINOR_UNIT_DIGITS[currency.toUpperCase()] ?? 2;
}

export function isCurrencyCode(value: string): boolean {
  return /^[A-Z]{3}$/u.test(value);
}

/** `-1234` in USD becomes `-$12.34`; formatting never touches the integer. */
export function formatMinorUnits(amount: number, currency: string, locale = "en-US"): string {
  const digits = minorUnitDigits(currency);
  const sign = amount < 0 ? "-" : "";
  const magnitude = Math.abs(amount);
  const whole = Math.trunc(magnitude / 10 ** digits);
  const fraction = magnitude - whole * 10 ** digits;
  try {
    const formatter = new Intl.NumberFormat(locale, {
      style: "currency",
      currency,
      minimumFractionDigits: digits,
      maximumFractionDigits: digits,
    });
    const value = whole + fraction / 10 ** digits;
    return `${sign}${formatter.format(value)}`;
  } catch {
    const fractionText = digits === 0 ? "" : `.${String(fraction).padStart(digits, "0")}`;
    return `${sign}${whole}${fractionText} ${currency}`;
  }
}

/** Decimal text such as `12.34` or `-0.5` to minor units; refuses anything else. */
export function parseAmount(text: string, currency: string): number {
  const digits = minorUnitDigits(currency);
  const trimmed = text.trim().replaceAll(",", "");
  const match = /^([+-])?(\d*)(?:\.(\d*))?$/u.exec(trimmed);
  if (match === null || (match[2] === "" && (match[3] ?? "") === "")) {
    throw new RangeError(`"${text}" is not an amount`);
  }
  const fractionText = match[3] ?? "";
  if (fractionText.length > digits) {
    throw new RangeError(`${currency} has ${digits} decimal places`);
  }
  const whole = match[2] === "" ? 0 : Number.parseInt(match[2] ?? "0", 10);
  const fraction =
    fractionText.length === 0 ? 0 : Number.parseInt(fractionText.padEnd(digits, "0"), 10);
  const magnitude = whole * 10 ** digits + fraction;
  if (!Number.isSafeInteger(magnitude)) {
    throw new RangeError(`"${text}" is too large`);
  }
  return match[1] === "-" ? -magnitude : magnitude;
}

/** Minor units back to editable decimal text (`-1234` → `-12.34`). */
export function amountToText(amount: number, currency: string): string {
  const digits = minorUnitDigits(currency);
  const sign = amount < 0 ? "-" : "";
  const magnitude = Math.abs(amount);
  const whole = Math.trunc(magnitude / 10 ** digits);
  const fraction = magnitude - whole * 10 ** digits;
  return digits === 0
    ? `${sign}${whole}`
    : `${sign}${whole}.${String(fraction).padStart(digits, "0")}`;
}
