from setuptools import setup
from setuptools.dist import Distribution


class BinaryDistribution(Distribution):
    """The native library is loaded with ctypes rather than linked as a CPython
    extension, so setuptools cannot infer that this wheel is platform-specific."""

    def has_ext_modules(self):
        return True


setup(distclass=BinaryDistribution)
