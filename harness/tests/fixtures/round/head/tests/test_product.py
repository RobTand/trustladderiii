import pytest
import product


def test_demonstrates():
    assert product.value() == 2


def test_does_not_demonstrate():
    assert True


def test_regression_passes():
    assert 2 + 2 == 4


def test_regression_breaks():
    assert product.value() == 1


def test_pre_existing():
    assert False


def test_preserved():
    assert product.value() > 0


@pytest.fixture
def ready():
    if product.value() == 1:
        raise RuntimeError("setup failed")


def test_setup(ready):
    assert True


def test_runtime_error():
    if product.value() == 1:
        raise RuntimeError("call failed without an assertion")


@pytest.mark.skip(reason="fixture")
def test_skipped():
    assert True


@pytest.mark.xfail(reason="fixture")
def test_xfailed():
    assert False


def uncollected():
    assert True


def test_new_name():
    assert product.new_name() == 2
