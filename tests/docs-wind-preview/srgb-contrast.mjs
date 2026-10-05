function relativeLuminance(rgb) {
  if (
    !Array.isArray(rgb) ||
    rgb.length < 3 ||
    rgb.slice(0, 3).some((channel) => !Number.isInteger(channel) || channel < 0 || channel > 255)
  ) {
    throw new TypeError("sRGB channels must be integer values from 0 to 255");
  }

  const linear = rgb.slice(0, 3).map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722;
}

export function contrastRatioSrgb(first, second) {
  const firstLuminance = relativeLuminance(first);
  const secondLuminance = relativeLuminance(second);
  const lighter = Math.max(firstLuminance, secondLuminance);
  const darker = Math.min(firstLuminance, secondLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}
