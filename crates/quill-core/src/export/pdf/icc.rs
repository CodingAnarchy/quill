//! The bundled grayscale ICC profile and the PDF/X output intent it feeds (#817).
//!
//! A print-ready novel interior is grayscale (black text, gray-flattened ornaments,
//! no transparency, fonts already subset + embedded), which makes it an ideal PDF/X
//! candidate. To reach conformance we add an `/OutputIntent` of subtype `/GTS_PDFX`
//! that names a *destination profile* — an embedded ICC characterizing the color the
//! file is prepared for. For a grayscale interior that profile is a calibrated
//! grayscale space, so the writer bundles [`GRAY_ICC`] and streams it as the output
//! intent's `/DestOutputProfile`.
//!
//! Conformance level reached: **PDF/X-3:2003**. PDF/X-3 permits calibrated
//! (ICC-based) grayscale alongside CMYK, which is exactly this interior — DeviceGray
//! samples plus a grayscale output intent, no uncalibrated RGB, no transparency, all
//! fonts embedded. (PDF/X-1a would demand a CMYK/gray *device* output intent and
//! forbids ICC-based colours in content; a gray interior with a gray output intent
//! satisfies PDF/X-3 without needing a press CMYK profile the author hasn't chosen.)
//!
//! [`GRAY_ICC`] is a minimal, public-domain grayscale display profile (gamma 2.2, D50
//! white point) generated for Quill — no third-party licence rides along in the export.

/// A minimal public-domain grayscale ICC profile (gamma 2.2, D50 white point),
/// embedded as the PDF/X output intent's destination profile (#817). Generated for
/// Quill; carries a "Public Domain" copyright tag so the exported PDF stays freely
/// distributable.
pub(super) const GRAY_ICC: &[u8] = include_bytes!("../../../../../assets/icc/QuillGray.icc");

/// The PDF/X version string recorded in `/Info` under `/GTS_PDFXVersion` and echoed by
/// `/GTS_PDFXConformance` (#817). A PDF/X reader keys off this to select validation.
pub(super) const PDFX_VERSION: &str = "PDF/X-3:2003";

/// The human-readable name of the output (print) condition the interior is prepared
/// for (#817), recorded on the output intent as `/OutputCondition` and, absent a
/// registered ICC characterization name, as `/OutputConditionIdentifier`.
pub(super) const OUTPUT_CONDITION: &str = "Quill Grayscale (gamma 2.2)";
