from pathlib import Path


def test_exception_stack_array_is_rendered_as_description_before_utf8():
    source = (Path(__file__).parents[1] / "src-tauri/src/crash_diagnostics.rs").read_text()
    assert "unsafe fn send_object(object: Id, selector: &[u8]) -> Id" in source
    assert "unsafe fn string_utf8(value: Id) -> String" in source
    assert 'let stack_array = send_object(exception, b"callStackSymbols\\0");' in source
    assert 'let stack_description = send_object(stack_array, b"description\\0");' in source
    assert "let stack = string_utf8(stack_description);" in source
    assert 'let name = string_utf8(send_object(exception, b"name\\0"));' in source
    assert 'let reason = string_utf8(send_object(exception, b"reason\\0"));' in source
    assert source.index('write_all(initial.as_bytes())') < source.index('b"callStackSymbols\\0"')
    assert 'catch_unwind(std::panic::AssertUnwindSafe' in source
