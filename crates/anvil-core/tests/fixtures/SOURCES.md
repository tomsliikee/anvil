# Test Fixtures

| Folder | Origin |
| :--- | :--- |
| **`doublyLinkedList/`** | A real course assignment written by **Visual Studio 2026** (`.slnx`, `net10.0`, StyleCop.Analyzers 1.1.118, `ClassDiagram1.cd`), used with Thomas Haiden's permission; `bin/`, `obj/`, `.vs/` left out. |
| **`sdk-samples/`** | Written by **.NET SDK 10.0.113** on Fedora: `dotnet new console`, `dotnet new classlib -o "My Lib"`, `dotnet new sln --format sln` / `dotnet new sln`, `dotnet sln add`. The SDK writes LF on Linux where VS writes CRLF. |
| **`moves/`** | Written by hand in Visual Studio's shape (BOM, CRLF, two-space indentation, backslash paths) to cover explicit items an SDK project can carry: `Compile Remove` with a glob, a `;` list, `Folder`, `None Update` with metadata. |
