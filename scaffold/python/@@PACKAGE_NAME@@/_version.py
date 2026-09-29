from importlib.metadata import PackageNotFoundError, version

try:
    __version__ = version("@@PACKAGE_NAME@@")
except PackageNotFoundError:
    __version__ = "0+uninstalled"
