// Validate each .docx named on the command line against the Office Open XML
// schemas (Office 2019) with the Open XML SDK, and list every error with the
// part and element it's in. Exits 1 if any file has an error (#1302).
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Validation;

var failed = 0;
foreach (var path in args)
{
    using var doc = WordprocessingDocument.Open(path, false);
    var errors = new OpenXmlValidator(FileFormatVersions.Office2019).Validate(doc).ToList();
    Console.WriteLine($"== Open XML: {Path.GetFileName(path)}: {errors.Count} errors");
    foreach (var error in errors.Take(50))
    {
        Console.WriteLine($"  {error.Part?.Uri} {error.Path?.XPath}: {error.Description}");
    }
    if (errors.Count > 0) failed++;
}
return failed == 0 ? 0 : 1;
