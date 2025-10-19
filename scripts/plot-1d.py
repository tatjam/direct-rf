#!/usr/bin/env python3
import numpy as np
import matplotlib.pyplot as plt
import sys

def plot_series(files):
    """
    Plot npy files as a series
    """
    
    plt.figure(figsize=(10, 8))
    
    for file in files:
        array= np.load(file)
        #array= 20 * np.log10(np.abs(array))
        array = np.fft.fftshift(array)
        max = array.argmax(axis=0)
        im = plt.plot(array) 
        #plt.axvline(x=max, color='red')
    
    plt.xlabel('Index')
    plt.ylabel('Value (dB)')
    
    plt.show()

if __name__ == "__main__":
    plot_series(sys.argv[1:])
