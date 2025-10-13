#!/usr/bin/env python3
import numpy as np
import matplotlib.pyplot as plt
import sys

def plot_csv_as_bitmap(file):
    """
    Plot a npy file as a bitmap/heatmap, with FFT frequency shifting
    """
    data = np.load(file)
    array_2d = np.fft.fftshift(data, axes=0)
    array_2d = 20 * np.log10(array_2d)
    
    plt.figure(figsize=(10, 8))
    
    im = plt.imshow(array_2d, 
                    cmap='viridis',
                    aspect='auto',
                    interpolation='nearest')
    
    plt.colorbar(im, label='Value')
    
    plt.xlabel('Column Index')
    plt.ylabel('Row Index')
    plt.title('2D Array Heatmap')
    
    plt.show()

if __name__ == "__main__":
    plot_csv_as_bitmap(sys.argv[1])
